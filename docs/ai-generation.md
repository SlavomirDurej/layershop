# AI generation (Window › AI Gen)

Fill a selection with an image from a generative model: a **local** model run by
[ComfyUI](https://github.com/comfyanonymous/ComfyUI), or a **cloud** model behind an API key
(OpenAI GPT Image, Google Gemini "Nano Banana").

PhotoCraft does not run models itself and bundles none. It builds the request, sends it to the
back end you set up, and places the answer on a new layer.

## Using it

1. **Edit › Preferences › AI Integrations…** has five pages:
   - **Local Models**: the list of local models beside the selected one's settings. **Scan
     Server** adds a model for every text-to-image model file the ComfyUI server has; **Add
     Model** adds one by architecture.
   - **Inpainting Models**: the models made for inpainting (below), with their own **Add
     Model** and **Scan Server**.
   - **Cloud Models**: the same for cloud models; it says which provider still lacks a key.
   - **API Keys**: one key per provider.
   - **Local Server**: the ComfyUI address, **Connect**, the timeout, and starting ComfyUI
     automatically (below).
2. **Window › AI Gen** — a tab next to Properties and Adjustments. Pick a model, write a prompt,
   press **Generate** (or ⌘/Ctrl+Return in the prompt). The prompt box fills the height of the
   panel: drag the edge of the dock group to make it taller. The gear opens that model's settings.
3. The result is a new layer over the **selection's bounds** (the whole canvas without a
   selection), named after the model and the prompt (`[flux_2_klein] a deer eating grass`). The selection becomes the layer's mask, so a lasso or feathered selection shapes
   the result and stays editable. One undo step; Esc or Cancel stops a running generation.

**Size.** The selection decides the shape. Models are trained near one megapixel and need edges
in multiples of 16, so a model renders at about its **Render size (MP)** in the selection's
aspect ratio, and the result is scaled to cover the selection and cropped to it. Set it to 0 to
render at the selection's own size. The panel shows what will be asked (`Selection 520 × 571 px
→ 976 × 1072 px`).

The tabs across the top of the panel choose what the model works from: **Txt2Img** (the prompt
alone, as above), **InPaint** and **Img2Img** (below). **ImgEdit** is shown but not available
yet.

**Alpha Channel** asks the model for a transparent background. It is available for models marked
*Supports a transparent background* in their settings (the GPT Image 2.5 presets have it on; the
Gemini models cannot). If a provider rejects the option, untick it there.

## Inpainting (the InPaint tab)

Repaints part of a picture: select what to replace on a pixel layer, describe what belongs
there, press **Generate**.

1. Choose the **pixel layer** that holds the image, and make a **selection** on it (Marquee,
   Lasso, a feathered selection). Both are needed: without a selection, or on a layer that is
   empty there or is not pixels (type, shape, adjustment, group), Generate is off and the panel
   says which.
2. The model is sent that layer around the selection and a mask of the selection. It paints
   inside the mask and sees the rest, so the light, perspective and colours carry over. For
   local models the mask fades in over the outer eighth of the selection: there the picture is
   kept more and more, which makes the model compose what it paints **inside** the selection
   (a subject is not cut off at its edge) and joins it to the picture without a seam.
3. The result is a **new layer** above, masked by the selection. The layer you selected on is
   not changed; hide or delete the new one to compare, or paint on its mask to blend the edge.

**The mask tools** under the prompt mark what to repaint without leaving the panel:

| Button | |
|---|---|
| Rectangle, Ellipse | the marquee tools: drag on the picture. Shift adds to the selection, Alt takes away |
| Brush | paint what to repaint. It shows as a red tint over the picture (it is Quick Mask, started empty, with the tint over the selected part), and starts from the selection there is |
| Eraser | paint out what should stay |
| − size + | the brush's size, for the brush and the eraser |
| Bin | clear the mask |

While the panel's brush is on, the Brush and Eraser paint the mask in and out whatever the
foreground and background colours are. **Generate** turns a painted mask into the selection
(switching to Rectangle or Ellipse, or pressing Q, does the same), and the selection becomes the
new layer's mask as before. Any other way of selecting works too: the Lasso, Select › Subject,
a saved selection.

- **Context** is how much of the layer around the selection the model is shown, as a share of
  the selection's longer edge on every side (25% by default). More helps it match the
  surroundings, but a model frames its subject in the whole picture it is given, so with a lot
  of context a subject can come out larger than the selection and be cut by it; less keeps it
  inside and leaves more of the rendered pixels for the selection. The panel
  shows the sizes (`Selection and context 720 × 760 px → 992 × 1040 px`).
- **Strength** is how far the result may depart from what is there. 100% paints the selection
  afresh; around 50–70% keeps its shapes and colours and changes the detail. Local models only.
- The InPaint tab lists the inpainting models and any other model ticked **Supports
  inpainting**, and remembers its own choice of model. Inpainting models are not offered on
  Txt2Img: they can't generate from a prompt alone.

**Which models.** Inpainting needs a model that knows where the selection is. Three are made
for it, the ones behind ComfyUI's own inpainting templates (in ComfyUI: **Templates ›
Inpainting** downloads their files); they live on the **Inpainting Models** page of the
settings, and **Scan Server** there adds the ones your server has files for:

| Model | Files | How it is run |
|---|---|---|
| FLUX.1 Fill Dev | `flux1-fill-dev`, two text encoders (`clip_l`, `t5xxl`), `ae` VAE | *Inpaint Model Conditioning* (picture and mask as conditioning), guidance 30, KSampler |
| FLUX.1 Fill OneReward | `flux.1-fill-dev-OneReward-transformer`, same encoders and VAE | the same graph. For object removal, set its LoRA to `removal_timestep_alpha-2-1740` and prompt what to remove |
| Qwen Image + InstantX ControlNet | `qwen_image_fp8_e4m3fn`, `qwen_2.5_vl_7b`, `qwen_image_vae`, and the ControlNet `Qwen-Image-InstantX-ControlNet-Inpainting` | *ControlNet Inpainting (AliMama) Apply* with the picture and mask, AuraFlow shift 3.1, CFG 2.5 |

They are given the selection as their mask as it is. **FLUX.2** (Klein, Dev) is not an
inpainting model but an edit model, and inpaints as well as they do: it is given the picture as a
reference (*Reference Latent*) and a mask that fades in from the selection's edge. It stays on
the Local Models page and is offered on both tabs.

Every other model can be tried by ticking **Supports inpainting** in its settings, and is run
by encoding the picture and denoising only the masked part (*Set Latent Noise Mask* with
*Differential Diffusion* and the fading mask). They are not told where the selection is, and it
shows. Tried against ComfyUI 0.39 with the same picture, selection and prompt (add an animal to
a meadow):

| Model | Result |
|---|---|
| FLUX.1 Fill Dev, FLUX.1 Fill OneReward, Qwen Image + InstantX, FLUX.2 Klein | the subject whole inside the selection, in the scene's light |
| Z-Image, Krea 2 | the subject inside the selection, on a background of its own |
| FLUX.1 Dev, Qwen Image | good at replacing a surface; a new subject brings its own surroundings |
| Ernie Image, HiDream I1 / O1, Ideogram 4 | seams, or a different scene in the box |

Instruction edit models (Qwen Image Edit, Z-Image Edit) have no built-in workflow: use a custom
workflow for them.

Cloud models are off for inpainting unless ticked. OpenAI's GPT Image models are sent the
picture and the mask (their image edit endpoint); Gemini has no mask input and is given the
picture and the mask as a second image. Both tend to repaint more than was asked; the new
layer's mask hides what falls outside the selection.

## Image to image (the Img2Img tab)

Redraws a picture following the prompt: the model starts from the picture instead of from noise,
so its composition carries over. The result is a new layer; the source is not changed.

- **Source** is the picture to redraw:
  - a **layer** of this document that has pixels (an empty layer is not offered). It is redrawn
    where it is: inside the selection when there is one, else over the layer's own pixels.
    Unasked it is the active layer.
  - another **open document**, as it shows (all its layers together; an empty document is not
    offered). It is fitted to the selection's bounds, or the whole canvas: scaled to cover the
    area and cropped to its shape.
- **Strength** is how far the result may depart from the picture (60% unasked). Low keeps its
  shapes and colours and changes the surface; high keeps little more than the composition; 100%
  ignores it. Local models only: it is how much of the sampler's schedule is run on the encoded
  picture (the *denoise*). Turbo models with a handful of steps move in coarse jumps: 60% of
  four steps is two.
- With a selection, the selection becomes the new layer's mask, as for a generated image.
- Every model that generates is offered (not the inpainting models). Cloud models are sent the
  picture and the prompt (OpenAI's image edit endpoint without a mask, Gemini with the picture
  attached) and have no strength.

Tried against ComfyUI 0.39 on one picture ("a watercolour painting of …"): Z-Image Turbo at 35%
keeps the picture and changes its surface, at 60% redraws it in the same layout; Krea 2 Turbo at
60% gives a watercolour of the same scene; FLUX.2 Klein (four steps) changes little at 60%.

## Starting ComfyUI for you

On the **Local Server** page, switch on **Start ComfyUI when a local model needs it and it isn't
running**. Then you don't open ComfyUI yourself: the first local generation starts it hidden
(the panel shows "Starting ComfyUI…", typically half a minute), later ones reuse it, and it is
stopped when PhotoCraft quits.

- **A ComfyUI that is already running is used as it is.** PhotoCraft first asks the address; if a
  ComfyUI answers (ComfyUI Desktop, a terminal), nothing is started, and nothing is stopped later:
  it is yours. Only a server PhotoCraft started is ever stopped (on quit, or with **Stop Server**).
- **Find Installation** fills in the Python, folder and arguments: from the ComfyUI that is
  running now (exactly how it was started), or, when none is running, from the installation
  ComfyUI Desktop recorded on this computer. The port comes from the address.
- **Browse…** is for any other installation (a git clone, the portable build): choose its
  `main.py` and its `python.exe`. Whichever half sits in a usual place next to the other
  (`.venv`, `venv`, `python_embeded`) is filled in for you. Outside Windows the Python path is
  typed, as it has no file type to browse by.
- What these buttons did is reported right under them.
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
| Qwen Image | diffusion model, text encoder (`qwen_image`, Qwen 2.5 VL), VAE | AuraFlow shift, KSampler |
| Qwen Image 2.1 | diffusion model, text encoder (`qwen_image`, Qwen 3 VL 8B), the 2.1 VAE | its own prompt node, KSampler; edges in multiples of 32 |
| FLUX.1 Fill (inpainting only) | diffusion model, two text encoders, VAE | Inpaint Model Conditioning, FLUX guidance, KSampler |
| Qwen Image + InstantX ControlNet (inpainting only) | diffusion model, text encoder (`qwen_image`), VAE, ControlNet | ControlNet Inpainting Apply, AuraFlow shift, KSampler |
| Checkpoint (SD / SDXL) | one checkpoint | KSampler |
| Custom workflow | whatever your workflow loads | yours |

Per model: steps, CFG, sampler, scheduler, shift, FLUX guidance, seed (-1: random each time),
negative prompt (used when CFG is above 1), an optional LoRA, and the render size.

**Custom workflow.** For anything the built-in graphs don't cover (LoRA stacks, prompt
enhancers, custom nodes): in ComfyUI use **Workflow › Export (API)** and paste the JSON. Put
`%prompt%`, `%negative%`, `%width%`, `%height%`, `%seed%`, `%steps%` and `%cfg%` where those
values go; a field that is exactly a numeric placeholder (`"%width%"`) becomes a number. The
workflow needs a Save Image or Preview Image node.

To inpaint with a custom workflow, give it two **Load Image** nodes whose `image` is `%image%`
and `%mask%`: PhotoCraft uploads the picture and the mask to ComfyUI's input folder (as
`photocraft_inpaint_image.png` and `photocraft_inpaint_mask.png`, replaced each time) and fills
in their names. The mask file is transparent where to paint, so the second node's **MASK**
output is the mask. `%denoise%` is the panel's Strength (1 for text to image).

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
| `ai.generate {"prompt", "mode": "inpaint", "strength"?, "context"?, …}` | repaint the selected part of the active pixel layer onto a new masked layer; needs a selection and pixels under it. `cargo run -p photocraft-engine --example ai_inpaint` runs it (and, with `img2img` in place of the rectangle, image to image) against a real server without the app |
| `ai.generate {"prompt", "mode": "img2img", "source"?: {"layer": id} or {"document": id}, "strength"?, …}` | redraw a layer in place, or another open document fitted to the area, onto a new layer |
| `ai.mask {"paint"?: bool, "clear"?: bool}` | paint the area to inpaint instead of selecting it: Quick Mask, started empty, tinted where selected; then `paint.stroke {"target": "quickMask", "color": "#ffffff"}` adds to it. `ai.generate` leaves the mode itself |
| `ai.sources` | what img2img can start from now: the active document's layers with pixels, the other open documents, and which is the default |
| `ai.models` | the models, whether each can run now, generates from a prompt (`generate`) and is offered for inpainting (`inpaint`) |
| `ai.comfyModels {"url"?}` | the model files a ComfyUI server has |
| `ai.server.status` / `ai.server.find` / `ai.server.stop` | the local server: state, how the running one was started, stop the one PhotoCraft started |
| `ai.setKey {"provider", "key"}` / `ai.keys` | set a key for this session / which providers have one |

Settings live at `integrations.ai` (`prefs.get` / `prefs.set`). The panel's state is
`ai_gen` in the UI state that `ui.inspect` returns (`mode`, `model`, `inpaintModel`, `strength`,
`context`, `maskBrush`, `imgSource`, `imgStrength`, `prompt`, `alpha`).

## Code map

- `crates/aigen` (L4): models and settings, the ComfyUI workflows (`comfy.rs`), cloud requests
  (`cloud.rs`), size rules (`size.rs`), model detection (`detect.rs`), and the one network seam,
  `Transport` (`http.rs`; `UreqTransport` on native, none on wasm). Everything but the transport
  is pure and tested without a network. Try a workflow against a real server with
  `cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188`; add
  `inpaint image.png mask.png [strength]` after the output file to try an inpainting.
- `crates/engine/src/ai_cmds.rs`: the commands. The app shell sets `Session::ai.transport`.
- `crates/ui-egui/src/ai_gen_ui.rs`: the panel and the Preferences › Integrations page.
