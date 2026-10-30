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
}

impl Ui {
    pub fn export(self, f: Format) {
        let (Some(mid), Some(m), Some(full)) = (self.mid.get_untracked(), self.model.get_untracked(), self.full.get_untracked()) else { return };
        if f == Format::Json {
            // The whole model, every diagram.
            let json = serde_json::to_string_pretty(&full).unwrap_or_default();
            return download(&format!("{mid}.json"), "application/json", &json);
        }
        // Images are of the diagram on screen.
        let name = format!("{mid}-{}", full.label(self.here()).to_lowercase().replace('.', "-"));
        if m.things.is_empty() {
            return self.say("Nothing to export yet");
        }
        // Everything drawn: things plus link detours, with a margin.
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for t in &m.things {
            let (w, h) = size(t);
            (x0, y0, x1, y1) = (x0.min(t.x), y0.min(t.y), x1.max(t.x + w + 5.0), y1.max(t.y + h + 5.0));
        }
        for g in self.routes.get_untracked().links.values() {
            (x0, y0, x1, y1) = (x0.min(g.bbox.0), y0.min(g.bbox.1), x1.max(g.bbox.2), y1.max(g.bbox.3));
        }
        let pad = 40.0;
        // Selection and hover highlights don't belong in the picture.
        self.select(None);
        self.hot.set(None);
        self.hover.set(vec![]);
        spawn_local(async move {
            TimeoutFuture::new(50).await;
            let Some(svg) = self.svg.get_untracked() else { return };
            let (png, ext) = if f == Format::Png { (true, "png") } else { (false, "svg") };
            export_image(&svg, x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad, png, &format!("{name}.{ext}"));
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
