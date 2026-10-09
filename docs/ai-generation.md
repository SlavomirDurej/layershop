# AI generation (Window › AI Gen)

Fill a selection with an image from a generative model: a **local** model run by
[ComfyUI](https://github.com/comfyanonymous/ComfyUI), or a **cloud** model behind an API key
(OpenAI GPT Image, Google Gemini "Nano Banana").

PhotoCraft does not run models itself and bundles none. It builds the request, sends it to the
back end you set up, and places the answer on a new layer.

## Using it

1. **Edit › Preferences › AI Integrations…** has four pages:
   - **Local Models**: the list of local models beside the selected one's settings. **Scan
     Server** adds a model for every text-to-image model file the ComfyUI server has; **Add
     Model** adds one by architecture.
   - **Cloud Models**: the same for cloud models; it says which provider still lacks a key.
   - **API Keys**: one key per provider.
   - **Local Server**: the ComfyUI address, **Connect**, the timeout, and starting ComfyUI
     automatically (below).
2. **Window › AI Gen** — a tab next to Properties and Adjustments. Pick a model, write a prompt,
   press **Generate** (or ⌘/Ctrl+Return in the prompt). The prompt box fills the height of the
   panel: drag the edge of the dock group to make it taller. The gear opens that model's settings.
3. The result is a new layer over the **selection's bounds** (the whole canvas without a
   selection). The selection becomes the layer's mask, so a lasso or feathered selection shapes
   the result and stays editable. One undo step; Esc or Cancel stops a running generation.

**Size.** The selection decides the shape. Models are trained near one megapixel and need edges
in multiples of 16, so a model renders at about its **Render size (MP)** in the selection's
aspect ratio, and the result is scaled to cover the selection and cropped to it. Set it to 0 to
render at the selection's own size. The panel shows what will be asked (`Selection 520 × 571 px
→ 976 × 1072 px`).

**Alpha Channel** asks the model for a transparent background. It is available for models marked
*Supports a transparent background* in their settings (the GPT Image 2.5 presets have it on; the
Gemini models cannot). If a provider rejects the option, untick it there.

## Starting ComfyUI for you

On the **Local Server** page, switch on **Start ComfyUI when a local model needs it and it isn't
running**. Then you don't open ComfyUI yourself: the first local generation starts it hidden
(the panel shows "Starting ComfyUI…", typically half a minute), later ones reuse it, and it is
stopped when PhotoCraft quits.

- **A ComfyUI that is already running is used as it is.** PhotoCraft first asks the address; if a
  ComfyUI answers (ComfyUI Desktop, a terminal), nothing is started, and nothing is stopped later:
  it is yours. Only a server PhotoCraft started is ever stopped (on quit, or with **Stop Server**).
- **Find Installation** fills in the Python, folder and arguments from the ComfyUI that is running
  now: start it once the way you usually do, press the button, and from then on PhotoCraft can
  start it the same way. The port comes from the address.
- Its output goes to `comfyui.log` in the settings folder; a start that fails quotes its last lines.
- If PhotoCraft crashes, a server it started keeps running. The next launch finds it answering
  and uses it, but no longer stops it on quit: close it from the Task Manager, or just leave it.
- If you open ComfyUI Desktop while PhotoCraft's server holds the port, Desktop has to pick
  another port or wait. Stop PhotoCraft's with **Stop Server** first.
- Agents (MCP / control channel) cannot change the launch settings: they name a program to run.

## Local models

ComfyUI must be running, or set up to be started (above). The portable build listens on port
8188, ComfyUI Desktop on 8000. Each
model has an **architecture**, which decides the workflow PhotoCraft queues, and the files it
loads. The workflows follow ComfyUI's own text-to-image templates and use only built-in nodes:

| Architecture | Loads | Sampling |
|---|---|---|
| Z-Image | diffusion model, text encoder (`lumina2`), VAE | AuraFlow shift, KSampler |
| Krea 2 | diffusion model, text encoder (`krea2`), VAE | KSampler |
| FLUX.2 (Klein, Dev) | diffusion model, text encoder (`flux2`), VAE | Flux2Scheduler, CFG guider, optional FLUX guidance |
| FLUX.1 | diffusion model, two text encoders, VAE | FLUX guidance, KSampler |
| Ernie Image | diffusion model, text encoder (`flux2`), VAE | KSampler |
| HiDream O1 | one checkpoint | noise scale, KSampler |
| HiDream I1 | diffusion model, four text encoders, VAE | SD3 shift, KSampler |
| Ideogram 4 | diffusion model + unconditional model, text encoder (`ideogram4`), VAE | AuraFlow shift, dual-model guider |
| Qwen Image | diffusion model, text encoder (`qwen_image`), VAE | AuraFlow shift, KSampler |
| Checkpoint (SD / SDXL) | one checkpoint | KSampler |
| Custom workflow | whatever your workflow loads | yours |

Per model: steps, CFG, sampler, scheduler, shift, FLUX guidance, seed (-1: random each time),
negative prompt (used when CFG is above 1), an optional LoRA, and the render size.

**Custom workflow.** For anything the built-in graphs don't cover (LoRA stacks, prompt
enhancers, custom nodes): in ComfyUI use **Workflow › Export (API)** and paste the JSON. Put
`%prompt%`, `%negative%`, `%width%`, `%height%`, `%seed%`, `%steps%` and `%cfg%` where those
values go; a field that is exactly a numeric placeholder (`"%width%"`) becomes a number. The
workflow needs a Save Image or Preview Image node.

## Cloud models

| Provider | Models | Size |
|---|---|---|
| OpenAI | `gpt-image-2.5-sunburst` (most capable), `gpt-image-2.5-flare` (fast), `gpt-image-2`: any size. The deprecated `gpt-image-1.5` / `gpt-image-1` still work, with three fixed sizes | from the selection's shape and Render size (MP) |
| Google Gemini | `gemini-3-pro-image-preview` (Nano Banana Pro), `gemini-3.1-flash-image-preview` (Nano Banana 2) | the closest supported aspect ratio at 1K / 2K / 4K |

**Cost (OpenAI).** Under Generate the panel shows what the image should cost at the model's
quality and the selection's size, and what the last one was billed (`Est. cost ≈ $0.041–$0.053
(medium) · last $0.047`). The billed amount is exact: it comes from the tokens OpenAI reports in
its answer. The estimate uses OpenAI's published figures, which cover the three sizes of the GPT
Image 1 family and of GPT Image 2 at low, medium and high. OpenAI publishes no formula for other
sizes and no figures for GPT Image 2.5 (only an interactive calculator), so those are marked
`≈`, and `xhigh` / `max` show the token rate instead of a guess. `ai.generate` returns the billed
`cost`. Rates live in `crates/aigen/src/pricing.rs`; update them when OpenAI's prices change.

The **Model ID** is editable, so a newer model of the same API works without an update.

**API keys** are stored in `ai-keys.json` in the settings directory, apart from
`preferences.json`: preferences can be read by agents (`prefs.get`), keys cannot (`ai.keys`
reports only whether one is set). Without a stored key the `OPENAI_API_KEY` / `GEMINI_API_KEY`
environment variables are used. A key is sent only to its provider. The file is plain text;
protect it like any other credential on the machine.

## For agents

Everything is a command (`photocraft-cli commands | grep '^ai\.'`):

| Command | |
|---|---|
| `ai.generate {"prompt", "model"?, "alpha"?, "seed"?, "rect"?, "mask"?}` | generate onto a new layer; a background job |
| `ai.models` | the models and whether each can run now |
| `ai.comfyModels {"url"?}` | the model files a ComfyUI server has |
| `ai.server.status` / `ai.server.find` / `ai.server.stop` | the local server: state, how the running one was started, stop the one PhotoCraft started |
| `ai.setKey {"provider", "key"}` / `ai.keys` | set a key for this session / which providers have one |

Settings live at `integrations.ai` (`prefs.get` / `prefs.set`). The panel's state is
`ai_gen` in the UI state that `ui.inspect` returns (`model`, `prompt`, `alpha`).

## Code map

- `crates/aigen` (L4): models and settings, the ComfyUI workflows (`comfy.rs`), cloud requests
  (`cloud.rs`), size rules (`size.rs`), model detection (`detect.rs`), and the one network seam,
  `Transport` (`http.rs`; `UreqTransport` on native, none on wasm). Everything but the transport
  is pure and tested without a network. Try a workflow against a real server with
  `cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188`.
- `crates/engine/src/ai_cmds.rs`: the commands. The app shell sets `Session::ai.transport`.
- `crates/ui-egui/src/ai_gen_ui.rs`: the panel and the Preferences › Integrations page.
