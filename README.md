# OPM Studio

Editor de OPM (Object-Process Methodology, ISO 19450) em Rust: canvas na web, OPL gerado ao vivo,
e o ChatGPT desenha os modelos via MCP.

- `core/` – modelo, regras de links da ISO, gerador de OPL, layout automático (`cargo test -p opm-core`)
- `server/` – axum: REST, WebSocket ao vivo, MCP em `/mcp/<token>`; modelos salvos em `data/*.json`
- `web/` – editor em Leptos (WASM)

## Rodar

Uma vez:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --version 0.21.14 --locked
```

Sempre que mudar a interface:

```sh
cd web && trunk build --release && cd ..     # ou `trunk watch` enquanto desenvolve
```

Para usar:

```sh
cargo run -p opm-server                      # abre em http://localhost:3000
```

Ao iniciar, o servidor imprime o endereço do editor e a URL do MCP (com o token).

## Editar

- `O` objeto, `P` processo, `S` estado; duplo clique renomeia (ou cria, no canvas vazio).
- Passe o mouse num item e arraste um dos pontinhos até outro item para ligar; solte no vazio para criar e ligar.
- Arraste o fundo para selecionar em caixa, `Shift`+clique para somar, `Espaço`+arrastar (ou botão do meio) para mover a tela.
- `Ctrl+C / X / V / D` copiar, recortar, colar (sob o cursor, entre modelos e abas), duplicar. `Ctrl+Z/Y` desfazer/refazer.
- Setas do teclado movem um pouco (`Shift` ×10), `F` enquadra, `T` organiza o diagrama (camadas com menos cruzamentos;
  num in-zoom, linha do tempo).
- `Del`: se o item aparece em outros diagramas, sai só deste; senão é apagado do modelo
  (o painel do item também tem "Delete from the whole model").
- Estilo das setas (reta / curva / ângulo reto) no canto inferior direito; em ângulo reto elas desviam dos itens.

## Diagramas (OPDs)

Um modelo é **um sistema só**: cada objeto, processo e ligação existe uma vez. Os diagramas são vistas
dele, numa árvore — **SD** (System Diagram) no topo, **SD1**, **SD2**, **SD1.1**… abaixo. Mudou um item,
mudou em todos os diagramas que o mostram.

- **Zoom in** (processo): abre um diagrama de detalhe onde o processo vira uma moldura grande com seus
  subprocessos dentro, **em ordem de tempo de cima para baixo**. O que o processo toca já vem junto, do lado de fora.
  Itens criados dentro da moldura viram parte dela; o painel do item tem *Inside / Outside* para mudar.
- **Unfold** (objeto ou processo): diagrama de detalhe com suas partes, atributos e especializações.
- Botões no menu flutuante do item ou no painel; **duplo clique** num item com detalhe (contorno grosso,
  como na notação OPM) abre o diagrama dele. Navegue pelo caminho no topo do canvas (SD › SD1…)
  ou pela lista **Diagrams** no painel — que também apaga um diagrama (e o que só ele mostra).
- Botão **Show an existing thing** (barra da esquerda): traz para este diagrama um item que já existe em outro.
- Num in-zoom, as ligações de fora com o processo aparecem na moldura até você ligá-las a um subprocesso.
- Painel **Checks**: avisa o que está desbalanceado ou mal formado, com o diagrama onde olhar:
  ligação do processo que nenhum subprocesso assume; subprocesso usando algo que o processo-pai não mostra
  no nível de cima; in-zoom com menos de dois subprocessos; processo que não transforma nenhum objeto;
  objeto sem ligação.
- OPL: *Copy* copia o diagrama atual; *All* copia todos, um após o outro.
- Modelos antigos (de antes dos diagramas) abrem com tudo no SD, sem perder nada.

## Modelos: renomear, exportar, importar, apagar

No botão **⋯** ao lado do nome do modelo:

- **Rename…** – muda o nome (o endereço `#id` continua o mesmo).
- **Export JSON** – baixa o modelo completo (todos os diagramas), para guardar ou reusar em outro PC.
- **Export SVG / PNG** – imagem do diagrama na tela (sem seleção nem pontinhos), com as cores do tema atual; PNG em 2x.
- **Import JSON…** – também dá para arrastar um `.json` exportado para qualquer lugar da página, ou usar
  *Import…* na tela inicial. Vira um modelo novo; se o nome já existir, ganha "(2)".
- **Delete model…** – pede confirmação e move o arquivo para `data/.trash/`. Para recuperar: mova o arquivo
  de volta para `data/` (tirando o sufixo numérico do nome) e reinicie o servidor.

Esses recursos só funcionam no editor (`localhost`). Pelo MCP, a IA pode apagar itens de um modelo, mas não
pode apagar modelos inteiros.

## Conectar ao ChatGPT

O ChatGPT precisa de um endereço HTTPS público. O **Tailscale Funnel** dá um endereço fixo e gratuito para este PC.

### Uma vez só

1. Instale e entre no Tailscale:
   ```sh
   winget install Tailscale.Tailscale
   ```
   Depois clique no ícone do Tailscale perto do relógio → *Log in*.
2. Ligue o Funnel (o `--bg` mantém ligado mesmo depois de reiniciar o PC):
   ```sh
   tailscale funnel --bg 3000
   ```
   Na primeira vez ele mostra um link para aprovar o Funnel na sua conta; aprove e ele continua sozinho.
   Seu endereço: `https://dell.tailc0d357.ts.net` (confira com `tailscale funnel status`).
3. No ChatGPT: **Settings → Apps & Connectors → Advanced → Developer mode** (ligar) → **Create connector**:
   - **URL:** `https://dell.tailc0d357.ts.net/mcp/<token>`
     (o token aparece ao iniciar o servidor, no botão *Connect ChatGPT* do editor e em `data/.mcp_token`)
   - **Authentication:** **No authentication** (não use OAuth — a proteção é o token na URL)

### No dia a dia

1. `cargo run -p opm-server` (o PC precisa estar ligado).
2. Numa conversa do ChatGPT, ative o conector e peça, por exemplo:
   *"Model an espresso machine in OPM Studio."*
3. O diagrama aparece ao vivo no editor (http://localhost:3000): itens novos surgem com um brilho,
   mudanças de posição deslizam e a tela se enquadra sozinha se algo cair fora da área visível.

O layout é automático: enquanto a IA desenha, o modelo inteiro é organizado em camadas. Itens que você
arrastar à mão ficam onde você deixou; dali em diante a IA só encaixa os itens novos perto dos vizinhos.
`T` (Tidy) reorganiza tudo e devolve o layout ao modo automático.

Ferramentas que o ChatGPT recebe: `list_models`, `create_model`, `add_things` (num diagrama: `diagram="SD1"`),
`add_links`, `zoom_in` e `unfold` (criam diagramas de detalhe), `remove` (do modelo, ou só de um diagrama),
`arrange` (reorganiza um diagrama inteiro) e `get_model` (árvore de diagramas, OPL de cada um e os checks).
O guia que a IA recebe pede para nunca omitir itens para "caber": diagramas grandes viram refinamentos.

### Segurança

- Pela internet só `/mcp/<token>` responde; o editor e a API devolvem 403 (só abrem em `localhost`).
- A URL com o token é a chave: quem tiver o link pode editar seus modelos. Não compartilhe.
- Trocar o token: apague `data/.mcp_token` (ou defina a variável `OPM_MCP_TOKEN`), reinicie o servidor
  e atualize a URL no conector do ChatGPT.
- Desligar o acesso pela internet: `tailscale funnel --https=443 off`.

## Conectar ao Claude Code (CLI)

O Claude Code roda neste PC, então usa `localhost` direto (sem Tailscale, sem autenticação):

```sh
claude mcp add --transport http --scope user opm http://localhost:3000/mcp/<token>
```

- `--scope user` deixa disponível em qualquer pasta (sem ele, só neste projeto).
- Conferir: `claude mcp list` (deve mostrar `opm` conectado) ou `/mcp` dentro do Claude Code.
- Peça, por exemplo: *"Modele uma máquina de café no OPM Studio"* — usa as mesmas ferramentas do ChatGPT.
- O servidor precisa estar rodando. Se trocar o token: `claude mcp remove opm` e adicione de novo.

## Se não funcionar

- `tailscale funnel status` deve mostrar `Funnel on` apontando para `http://127.0.0.1:3000`.
- O servidor precisa estar rodando: http://localhost:3000 abre o editor?
- Testar o MCP sem o ChatGPT: `npx @modelcontextprotocol/inspector` com a URL
  `http://localhost:3000/mcp/<token>` (transporte *Streamable HTTP*).
