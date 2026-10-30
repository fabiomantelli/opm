//! Export and import: model JSON, diagram images (see files.js), and the model menu's server calls.
use crate::{e2s, fetch_json, list_models, Ui};
use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use leptos::{prelude::*, task::spawn_local};
use opm_core::layout::size;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/src/files.js")]
extern "C" {
    fn download(name: &str, mime: &str, text: &str);
    #[wasm_bindgen(js_name = svgMarkup)]
    fn svg_markup(svg: &web_sys::Element, x: f64, y: f64, w: f64, h: f64) -> String;
    #[wasm_bindgen(js_name = exportImage)]
    fn export_image(svg: &web_sys::Element, x: f64, y: f64, w: f64, h: f64, png: bool, name: &str);
    #[wasm_bindgen(js_name = pickFile)]
    fn pick_file(accept: &str, done: JsValue);
    #[wasm_bindgen(js_name = onFileDrop)]
    fn on_file_drop(done: JsValue);
}

#[derive(Clone, Copy, PartialEq)]
pub enum Format {
    Json,
    Svg,
    Png,
    SysMl,
}

/// Text for HTML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

const REPORT_CSS: &str = "body{font:15px/1.5 system-ui,-apple-system,Segoe UI,sans-serif;color:#1b1d23;max-width:960px;margin:40px auto;padding:0 20px}
h1{font-size:30px;margin:0 0 4px;letter-spacing:-.02em}h2{margin-top:40px;border-bottom:1px solid #ddd;padding-bottom:6px}
.meta{color:#666;margin:0}.fig{border:1px solid #ddd;border-radius:10px;padding:8px;margin:12px 0;text-align:center}
.fig svg{max-width:100%;height:auto}ol.opl{padding-left:22px}table{border-collapse:collapse;width:100%}
th,td{border:1px solid #ddd;padding:6px 10px;text-align:left;vertical-align:top}th{background:#f5f5f7}
.missing{color:#b42318}.toc a{color:inherit}section{break-inside:avoid-page}
@media print{body{margin:0;max-width:none}h2{break-before:page}h2.first{break-before:auto}}";

impl Ui {
    pub fn export(self, f: Format) {
        let (Some(mid), Some(m), Some(full)) = (self.mid.get_untracked(), self.model.get_untracked(), self.full.get_untracked()) else { return };
        if f == Format::Json {
            // The whole model, every diagram.
            let json = serde_json::to_string_pretty(&full).unwrap_or_default();
            return download(&format!("{mid}.json"), "application/json", &json);
        }
        if f == Format::SysMl {
            return download(&format!("{mid}.sysml"), "text/plain", &opm_core::sysml::export(&full));
        }
        // Images are of the diagram on screen.
        let name = format!("{mid}-{}", full.label(self.here()).to_lowercase().replace('.', "-"));
        if m.things.is_empty() {
            return self.say("Nothing to export yet");
        }
        self.clear_highlights();
        spawn_local(async move {
            TimeoutFuture::new(50).await;
            let (Some(svg), Some((x, y, w, h))) = (self.svg.get_untracked(), self.drawn_box()) else { return };
            let (png, ext) = if f == Format::Png { (true, "png") } else { (false, "svg") };
            export_image(&svg, x, y, w, h, png, &format!("{name}.{ext}"));
        });
    }

    /// Selection and hover highlights don't belong in a picture.
    fn clear_highlights(self) {
        self.select(None);
        self.hot.set(None);
        self.hover.set(vec![]);
    }

    /// Everything drawn on the canvas (things plus link detours), with a margin: (x, y, w, h).
    fn drawn_box(self) -> Option<(f64, f64, f64, f64)> {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        self.model.with_untracked(|m| {
            for t in m.iter().flat_map(|m| &m.things) {
                let (w, h) = size(t);
                (x0, y0, x1, y1) = (x0.min(t.x), y0.min(t.y), x1.max(t.x + w + 5.0), y1.max(t.y + h + 5.0));
            }
        });
        for g in self.routes.get_untracked().links.values() {
            (x0, y0, x1, y1) = (x0.min(g.bbox.0), y0.min(g.bbox.1), x1.max(g.bbox.2), y1.max(g.bbox.3));
        }
        let pad = 40.0;
        (x0 < x1).then(|| (x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad))
    }

    /// One HTML file with every diagram (drawn in the light theme), its OPL, the requirements and their
    /// traces, and the checks: to read, share, or print to PDF.
    pub fn report(self) {
        let (Some(mid), Some(full)) = (self.mid.get_untracked(), self.full.get_untracked()) else { return };
        self.clear_highlights();
        self.say("Building the report…");
        spawn_local(async move {
            let root = document().document_element().unwrap();
            let theme = root.get_attribute("data-theme");
            let _ = root.set_attribute("data-theme", "light");
            let mut sections = String::new();
            let mut toc = String::new();
            for (i, (opd, depth)) in full.tree().into_iter().enumerate() {
                let view = full.view(opd);
                let opl: String = opm_core::opl::render(&view).iter().map(|s| format!("<li>{}</li>", esc(&s.text))).collect();
                self.model.set(Some(view));
                TimeoutFuture::new(120).await; // routes and DOM catch up
                let svg = match (self.svg.get_untracked(), self.drawn_box()) {
                    (Some(svg), Some((x, y, w, h))) => svg_markup(&svg, x, y, w, h),
                    _ => "<p>(empty diagram)</p>".into(),
                };
                let title = esc(&full.title(opd));
                toc += &format!("<li style=\"margin-left:{}px\"><a href=\"#d{opd}\">{title}</a></li>", depth * 18);
                let first = if i == 0 { " class=\"first\"" } else { "" };
                sections += &format!("<section id=\"d{opd}\"><h2{first}>{title}</h2><div class=\"fig\">{svg}</div><h3>OPL</h3><ol class=\"opl\">{opl}</ol></section>");
            }
            match theme {
                Some(t) => {
                    let _ = root.set_attribute("data-theme", &t);
                }
                None => {
                    let _ = root.remove_attribute("data-theme");
                }
            }
            self.show_here();

            let mut reqs = String::new();
            for r in &full.requirements {
                let by: Vec<String> = r
                    .satisfied_by
                    .iter()
                    .filter_map(|&t| full.thing(t))
                    .map(|t| format!("{} <small>({})</small>", esc(&t.name), full.shown_in(t.id).iter().map(|&o| full.label(o)).collect::<Vec<_>>().join(", ")))
                    .collect();
                let by = if by.is_empty() { "<span class=\"missing\">not satisfied yet</span>".to_string() } else { by.join("<br>") };
                reqs += &format!("<tr><td>{}</td><td>{}</td><td>{by}</td></tr>", esc(&r.key), esc(&r.text));
            }
            let reqs = if reqs.is_empty() { "<p>No requirements recorded.</p>".to_string() } else { format!("<table><tr><th>Key</th><th>Requirement</th><th>Satisfied by</th></tr>{reqs}</table>") };
            let issues = opm_core::check::check(&full);
            let checks = if issues.is_empty() {
                "<p>Balanced and well-formed.</p>".to_string()
            } else {
                format!("<ul>{}</ul>", issues.iter().map(|i| format!("<li><b>{}</b> {}</li>", full.label(i.opd), esc(&i.text))).collect::<String>())
            };
            let date: String = web_sys::js_sys::Date::new_0().to_locale_date_string("default", &wasm_bindgen::JsValue::UNDEFINED).into();
            let name = esc(&full.name);
            let html = format!(
                "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{name} – OPM model</title><style>{REPORT_CSS}</style></head><body>\
                 <h1>{name}</h1><p class=\"meta\">OPM model (ISO 19450) · {date} · {} diagrams · {} things · {} requirements</p>\
                 <h2 class=\"first\">Contents</h2><ul class=\"toc\" style=\"list-style:none;padding:0\">{toc}</ul>{sections}\
                 <h2>Requirements</h2>{reqs}<h2>Checks</h2>{checks}</body></html>",
                full.opds.len(),
                full.things.len(),
                full.requirements.len(),
            );
            download(&format!("{mid}-report.html"), "text/html", &html);
            self.say("Report downloaded: open it to read, or print it to PDF");
        });
    }

    /// Opens a file chooser for a model .json and imports it.
    pub fn import_pick(self) {
        pick_file(".json,application/json", Closure::once_into_js(move |text: String| self.import(text)));
    }

    /// Dropping a model .json anywhere imports it.
    pub fn import_on_drop(self) {
        on_file_drop(Closure::<dyn FnMut(String)>::new(move |text: String| self.import(text)).into_js_value());
    }

    fn import(self, text: String) {
        spawn_local(async move {
            let req = Request::post("/api/import").header("Content-Type", "application/json").body(text);
            match fetch_json::<String>(req).await {
                Ok(id) => {
                    self.models.set(list_models().await);
                    let _ = window().location().set_hash(&id);
                    self.say("Imported");
                }
                Err(e) => self.say(format!("Not an OPM Studio model: {e}")),
            }
        });
    }

    pub fn rename_model(self, name: String) {
        let Some(mid) = self.mid.get_untracked() else { return };
        spawn_local(async move {
            let res = match Request::post(&format!("/api/models/{mid}/rename")).json(&serde_json::json!({ "name": name })) {
                Ok(r) => r.send().await.map_err(e2s).and_then(|r| if r.ok() { Ok(()) } else { Err(format!("HTTP {}", r.status())) }),
                Err(e) => Err(e2s(e)),
            };
            match res {
                Ok(()) => self.models.set(list_models().await),
                Err(e) => self.say(e),
            }
        });
    }

    /// The server keeps it in data/.trash.
    pub fn delete_model(self) {
        let Some(mid) = self.mid.get_untracked() else { return };
        spawn_local(async move {
            match Request::delete(&format!("/api/models/{mid}")).send().await {
                Ok(r) if r.ok() => {
                    let _ = window().location().set_hash("");
                    self.models.set(list_models().await);
                    self.say("Model moved to data/.trash");
                }
                Ok(r) => {
                    let status = r.status();
                    let text = r.text().await.unwrap_or_default();
                    self.say(if text.is_empty() { format!("Couldn't delete (HTTP {status}); restart the server if it's outdated") } else { text });
                }
                Err(e) => self.say(e2s(e)),
            }
        });
    }
}
