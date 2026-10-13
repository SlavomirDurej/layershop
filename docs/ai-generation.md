# AI generation (Window › AI Gen)

Fill a selection with an image from a generative model: a **local** model run by
[ComfyUI](https://github.com/comfyanonymous/ComfyUI), or a **cloud** model behind an API key
(OpenAI GPT Image, Google Gemini "Nano Banana", xAI Grok Imagine, Meta Muse Image, Black Forest
Labs FLUX).

PhotoCraft does not run models itself and bundles none. It builds the request, sends it to the
back end you set up, and places the answer on a new layer.

## Using it

1. **Edit › Preferences › Generative AI…** (a section of its own in the Preferences dialog, under
   Integrations) has three pages:
   - **Manage Models**. At the top, what is set up: **Cloud models** and **Local models**, each
     with its count, what is worth knowing now (a provider without a key; whether ComfyUI
     answers) and a tick box that offers that kind in the AI Gen panel or leaves it out.
     **+ Add Model**, on the right, adds a cloud model, a local one by architecture, or one of
     the models made for inpainting. Below, every model in one list beside the selected one's
     settings: cloud models first, then local, each in your order, which is the order of the
     panel's model list. Under each name the list says what the model is used for (*Text to
     image*, *Editing*, *Inpainting*), and the filters above it (**All**, **Text to Image**,
     **Editing**, **Inpainting**, each with its count) narrow it to one of those. The selected
     row has arrows that move it up or down among its kind. Under the list: **Scan Server** adds
     a model for every model file the ComfyUI server has; **Restore Defaults** puts back the
     recommended models that are missing, and changes nothing else; **Recommended Order** sorts
     the list best and newest first. A local model whose file the server doesn't have is marked.
   - **API Keys**: one key per provider.
   - **Local Server**: the ComfyUI address, **Connect**, the timeout, and starting ComfyUI
     automatically (below).
2. **Window › AI Gen** — a tab next to Properties and Adjustments. It has two tabs of its own:
   **Create** makes a new image from a prompt, **Edit** changes a picture that is there (below).
   Pick a model, write a prompt, press **Generate** (or ⌘/Ctrl+Return in the prompt). The prompt
   box fills the height of the panel: drag the edge of the dock group to make it taller. The gear
   opens that model's settings.
3. The result is a new layer over the **selection's bounds** (the whole canvas without a
   selection), named after the model and the prompt (`[flux_2_klein] a deer eating grass`). The selection becomes the layer's mask, so a lasso or feathered selection shapes
   the result and stays editable. One undo step; Esc or Cancel stops a running generation.

4. A generated layer **remembers what it was made with**: the model, the prompt, what was
   done (created, a selection filled, changed by instruction, restyled) and its settings
   (strength, context, the source picture, alpha), and the seed. Click the layer in the Layers
   panel (its name, thumbnail or mask) and the AI Gen panel loads them back, on the tab and task
   they belong to, ready to generate again or to change. The seed is not loaded: generating
   again gives a new image. The eye does not load anything: showing and hiding a layer leaves
   the panel alone. So do ⌘/Ctrl- and Shift-clicks, which select several layers. If the model
   has since been removed or disabled (or no longer does what the layer was made with), the
   panel keeps the model it had and loads the rest. Nothing is loaded
   while a generation is running. The settings are saved in `.pcraft` files, in PSD and PSB files and in
   layered TIFFs, and copied with the layer (Duplicate Layer); flat formats (PNG, JPEG…) don't
   keep them. In a PSD they are one item (`pcGn`, JSON) of the layer's `shmd` metadata block,
   next to the items Photoshop keeps there: other programs step over it, so the file opens as
   any PSD does. Whether Photoshop writes the item back when it saves the file again has not
   been checked; if it does not, the settings are gone from that copy.

**Size.** The selection decides the shape. Models are trained near one megapixel and need edges
in multiples of 16, so a model renders at about its **Render size (MP)** in the selection's
aspect ratio, and the result is scaled to cover the selection and cropped to it. Set it to 0 to
render at the selection's own size. The panel shows what will be asked (`Selection 520 × 571 px
→ 976 × 1072 px`). Ming Image is the exception: it renders at two megapixels (see *Ming Image*
under *Local models*).

## Create and Edit

The two tabs across the top of the panel say what you are there to do.

**Create** generates a new image from the prompt alone, as above. It lists the models that
generate.

**Edit** changes a picture that is there. The row under the tabs chooses the task:

- **Change** changes the picture as the prompt says. With a selection only that part changes, and
  the rest stays exactly as it was; without one the whole picture is changed. It lists the models
  that edit or inpaint, and how the model is asked follows from the model:
  - an **edit model** (Qwen Image 2.1, Ming Image, Qwen Image Edit, FLUX.2, the cloud models) is shown the
    picture and told what to change: "make the jacket red leather". See *Changing by
    instruction*.
  - an **inpainting model** (FLUX.1 Fill, Qwen Image with the InstantX ControlNet, or a model
    ticked **Supports inpainting**) is given the selection as its mask and paints what the prompt
    describes inside it: "a deer eating grass". It needs a selection. See *Filling a selection*.
  - a model that does both (FLUX.2; a cloud model ticked **Supports inpainting**) is asked by
    instruction unless **Advanced › Method** says **Fill the selection**.

  The hint in the prompt box says which it is: "Say what to change…" or "Describe what to paint
  in the selection…".
- **Restyle** redraws the picture following the prompt, keeping its composition. See *Restyling*.

Change has its own choice of model, as it lists other models than Create and Restyle, which
share theirs.

**The mask tools** under the prompt of a change mark what to change without leaving the panel
(*Filling a selection* describes them). They work for an instruction as well: what is marked
scopes the change.

**Advanced**, above Generate, opens the settings few generations need:

| Setting | For | |
|---|---|---|
| Method | a change, with a model that does both | **By instruction** or **Fill the selection** |
| Strength | a fill | how far the result may depart from what is there |
| Quality | a change by instruction | how much of a local model's schedule to run |
| Context | a change | how much of the picture around the selection the model is shown |
| Seed | everything | the number the image starts from. Empty: the model's own setting, a new seed each time unless it has a fixed one. The lock keeps the seed of the last image, so the next one differs only by what you change; click it again for a new seed each time |

Steps, CFG, sampler, scheduler and the other rendering parameters are per model, in its settings
(the gear).

**Alpha Channel** asks the model for a transparent background. It is available for models marked
*Supports a transparent background* in their settings (the GPT Image 2.5 presets have it on; the
Gemini models cannot). If a provider rejects the option, untick it there. Of the local models
**Ming Image** has it on: see *Ming Image* under *Local models*.

## Filling a selection (Edit › Change, with an inpainting model)

Repaints part of a picture: on **Edit › Change**, with an inpainting model chosen, select what
to replace on a pixel layer, describe what belongs there, press **Generate**.

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

- **Context** (Advanced) is how much of the layer around the selection the model is shown, as a share of
  the selection's longer edge on every side (25% by default). More helps it match the
  surroundings, but a model frames its subject in the whole picture it is given, so with a lot
  of context a subject can come out larger than the selection and be cut by it; less keeps it
  inside and leaves more of the rendered pixels for the selection. The panel
  shows the sizes (`Selection and context 720 × 760 px → 992 × 1040 px`).
- **Strength** (Advanced) is how far the result may depart from what is there. 100% paints the
  selection afresh; around 50–70% keeps its shapes and colours and changes the detail. Local
  models only.
- Change lists the inpainting models and any other model ticked **Supports inpainting**, next to
  the edit models. Inpainting models are not offered on Create or for Restyle: they can't
  generate from a prompt alone.

**Which models.** Inpainting needs a model that knows where the selection is. Three are made
for it, the ones behind ComfyUI's own inpainting templates (in ComfyUI: **Templates ›
Inpainting** downloads their files); the **Inpainting** filter of Manage Models
lists them, **Add Model** offers them, and **Scan Server** adds the ones your server has files for:

| Model | Files | How it is run |
|---|---|---|
| FLUX.1 Fill Dev | `flux1-fill-dev`, two text encoders (`clip_l`, `t5xxl`), `ae` VAE | *Inpaint Model Conditioning* (picture and mask as conditioning), guidance 30, KSampler |
| FLUX.1 Fill OneReward | `flux.1-fill-dev-OneReward-transformer`, same encoders and VAE | the same graph. For object removal, set its LoRA to `removal_timestep_alpha-2-1740` and prompt what to remove |
| Qwen Image + InstantX ControlNet | `qwen_image_fp8_e4m3fn`, `qwen_2.5_vl_7b`, `qwen_image_vae`, and the ControlNet `Qwen-Image-InstantX-ControlNet-Inpainting` | *ControlNet Inpainting (AliMama) Apply* with the picture and mask, AuraFlow shift 3.1, CFG 2.5 |

They are given the selection as their mask as it is. **FLUX.2** (Klein, Dev) is not an
inpainting model but an edit model, and inpaints as well as they do: it is given the picture as a
reference (*Reference Latent*) and a mask that fades in from the selection's edge. The list shows
it under all three filters. A change with it is by instruction; to have it fill the selection this
way, set **Advanced › Method** to **Fill the selection**.

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

## Restyling (Edit › Restyle)

Redraws a picture following the prompt (image to image): the model starts from the picture
instead of from noise, so its composition carries over. The result is a new layer; the source is
not changed.

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

## Changing by instruction (Edit › Change, with an edit model)

Changes a picture the way you say: "make the jacket red leather", "replace the apples with
oranges", "put the scarf from image 2 around her neck". An edit model is shown the picture whole
and renders a new one from it; the result is a new layer, and the source is not changed.

- **Source** is the picture to change ("image 1" to the model): a **layer** with pixels (the
  active one unasked), **This document (all layers)**, or another **open document** (fitted to
  the selection or the canvas, as for a restyle).
- **A selection scopes the edit** (crop, edit, composite). Only the selection's bounds, with a
  quarter of their longer edge around them for context, are sent to the model, and its answer
  goes back there on a layer masked by the selection. So everything outside the selection stays
  exactly as it was (edit models redraw the whole picture they are given, and it drifts a
  little), and a large picture is edited near its own resolution instead of being rendered at a
  megapixel and scaled back up. Feather the selection for a soft join. Without a selection the
  whole source is sent and the result has no mask. The mask tools under the prompt mark the
  part to change as well: a painted mask becomes the selection when you press Generate.
  **Context** (Advanced) is how much around the selection is sent, a quarter of its longer edge
  unasked.
- **Image 2, Image 3, …** are pictures the instruction may refer to, chosen from the same list
  (open a file to refer to it). Name them in the instruction as the rows do. Up to three; Qwen
  Image Edit takes two. They are sent at their own size, brought to between a quarter of a
  megapixel and one.
- **Quality** (Advanced) is how much of a local model's schedule to run: **Draft** about a quarter of its
  steps (four at least), to try an idea in seconds; **Standard** the steps it is set up with;
  **Detail** a fifth more, for hands and fine detail. A cloud model's quality is in its settings.
- **Alpha Channel** as on Create (cloud models that support it; an edit with Ming Image comes
  back opaque).
- The models offered are the ones that edit: **Qwen Image 2.1**, **Ming Image** and **FLUX.2**
  (which also generate), **Qwen Image Edit** 2509 / 2511 (which do nothing else; Scan Server adds them, with
  their four-step Lightning LoRA when the server has it), and cloud models (OpenAI's image edit
  endpoint, Gemini with the pictures attached).

Tried against ComfyUI 0.39 on one 1232 x 816 picture (RTX 4090): Qwen Image 2.1 recolours a
blouse in 16 s and replaces the fruit in a selected bowl in 11 s, with nothing outside the
selection changed; with a reference ("the scarf from image 2") a Draft takes 9 s. FLUX.2 Klein
(four steps) does the selected replacement in 11 s. Qwen Image Edit 2509 recolours the blouse in
19 s but shifts the whole picture a little, which is what the selection is for. Qwen Image Edit
2511 (the 38 GB bf16 file) does the selected replacement in 41 s, loading included; it shifts
the picture too, and with a hard-edged selection that shows as a seam along the selection's
edge where the picture has detail. Feather the selection with those two models.

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

## The recommended models

A new installation lists fifteen text-to-image models, each with the settings it was judged at:
one prompt (a coffee-shop scene with two people) rendered by every model at 1920 x 1080 on
2026-10-13, and the ones whose result was acceptable kept. `Model::defaults()` in
`crates/aigen/src/model.rs` is the list.

They are in the **recommended order**, best and newest first, which is the order of the AI Gen
panel's model list; the first is the one `ai.generate` uses when no model is named. Cloud
models come before local ones, in the order of [Artificial Analysis' text-to-image
board](https://artificialanalysis.ai/image/leaderboard/text-to-image) on 2026-10-13; the local
ones follow as that comparison left them (our own judgement: the board ranks one of them).
`Model::rank()` holds the order, also for models that are not listed by default, and
**Recommended Order** applies it to a list that has drifted.

| Model | Runs | Settings |
|---|---|---|
| GPT Image 2.5 Sunburst | OpenAI | quality auto, 1 MP |
| GPT Image 2.5 Flare | OpenAI | quality auto, 1 MP |
| Grok Imagine Image 2.0 | xAI | quality auto, 1K |
| Nano Banana 2 (`gemini-3.1-flash-image-preview`) | Google Gemini | 1K |
| FLUX 3 Image | Black Forest Labs | 1K |
| Muse Image | Meta | (none to set) |
| Nano Banana Pro (`gemini-3-pro-image-preview`) | Google Gemini | 1K |
| Qwen Image 2.1 | local | 25 steps, CFG 1, 1 MP |
| Krea 2 Turbo | local | 8 steps, CFG 1, 1 MP |
| Z-Image Turbo | local | 8 steps, CFG 2, `res_multistep`, shift 3, 1 MP |
| FLUX.2 Klein 9B (distilled) | local | 4 steps, CFG 1, 1 MP |
| Ideogram 4 | local | 28 steps, CFG 5, shift 5, 1 MP |
| HiDream O1 | local | 40 steps, CFG 5, `dpmpp_2m_sde_gpu`, noise scale 8, its trained sizes; prompt written by Gemma 4, result refined by Z-Image Turbo at 0.35 |
| Ernie Image Turbo | local | 8 steps, CFG 1, 2 MP |
| Ming Image 0.1 Design | local | 12 steps, CFG 1, 2 MP |

**Leaving a kind out.** Untick **Cloud models** or **Local models** at the top of Manage Models
and the panel's model lists (Create and Edit) no longer offer that kind; the models stay set up.
`integrations.ai.showCloud` / `showLocal` are the settings. A model of a kind that is left out
still runs when `ai.generate` names it.

Two of these differ from ComfyUI's templates: Z-Image Turbo's CFG is 2 (the template's is 1) and
Ideogram 4's is 5 (the template's is 7). The architecture presets and **Scan Server** use the
same values, so a model added either way gets them.

The local entries name the files that comparison ran (`z_image_turbo_int8_convrot`,
`krea2_turbo_int8_convrot`, …). A server often has the same model stored another way (`bf16`,
`fp8_scaled`): **Scan Server** then points the listed entry at the server's file and keeps its
settings, instead of listing the model twice ("1 matched to the server's files"). Until then
the list marks the entry *File not on the server*.

Preferences saved before have their own model list and keep it; **Restore Defaults** adds the
recommended models such a list is missing.

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
| HiDream O1 | one checkpoint | noise scale, patch seam smoothing, KSampler; edges in multiples of 32; see below |
| HiDream I1 | diffusion model, four text encoders, VAE | SD3 shift, KSampler |
| Ideogram 4 | diffusion model + unconditional model, text encoder (`ideogram4`), VAE | AuraFlow shift, dual-model guider |
| Qwen Image | diffusion model, text encoder (`qwen_image`, Qwen 2.5 VL), VAE | AuraFlow shift, KSampler |
| Qwen Image 2.1 | diffusion model, text encoder (`qwen_image`, Qwen 3 VL 8B), the 2.1 VAE | its own prompt node, KSampler; edges in multiples of 32 |
| Ming Image (0.1 Design) | diffusion model, text encoder (`qwen_image`, Ling mini 2.0), its own VAE | FLUX shift by size, a guider without CFG, 12 steps; see below |
| FLUX.1 Fill (inpainting only) | diffusion model, two text encoders, VAE | Inpaint Model Conditioning, FLUX guidance, KSampler |
| Qwen Image + InstantX ControlNet (inpainting only) | diffusion model, text encoder (`qwen_image`), VAE, ControlNet | ControlNet Inpainting Apply, AuraFlow shift, KSampler |
| Checkpoint (SD / SDXL) | one checkpoint | KSampler |
| Custom workflow | whatever your workflow loads | yours |

Per model: steps, CFG, sampler, scheduler, shift, FLUX guidance, seed (-1: random each time),
negative prompt (used when CFG is above 1), an optional LoRA, and the render size.

**Ideogram 4 refuses some seeds.** On some seeds, for harmless prompts too, the model draws a
flat grey picture reading "Image blocked by safety filter" in place of the image. The words are
drawn by the weights: the graph has no filter node, and ComfyUI and PhotoCraft have none. The same
prompt on another seed gives a normal image (seen 2026-10-12: one seed refused, the next did not).
PhotoCraft recognises the picture (mid-grey, about 96–98% of the pixels flat, a little light type,
no colour; real images are at most 20% flat), never puts it on a layer, and renders again on a
new random seed, up to three more times, saying so in the progress line. If every seed is refused,
or the seed was fixed in the model's settings or the `seed` parameter, `ai.generate` fails with an
error that says so and the document is untouched. Rewording the prompt is the only other remedy.

**HiDream O1** follows ComfyUI's two HiDream O1 templates (2026-10-12):

- **The full model** (`hidream_o1_image_bf16`, `…_mxfp8`): 40 steps, CFG 5, `dpmpp_2m_sde_gpu`,
  the `normal` scheduler, noise scale 8, and Patch Seam Smoothing over the last fifth of the
  schedule (the model works on pixels in patches, whose edges can show).
- **Dev** (a checkpoint with `dev` in its name, distilled): 28 steps, CFG 1 (no negative
  prompt), noise scale 7.6, the LCM sampler with its noise clipped at 2.5, and no smoothing.
  Choosing another sampler for it samples it as the full model is.
- **It renders at the sizes it was trained at**, about four megapixels each: 2048 × 2048,
  2304 × 1728, 2560 × 1440, 2496 × 1664, 3104 × 1312, 2304 × 1792 and their upright forms. A
  text-to-image gets the one closest to the selection's shape, whatever **Render size (MP)**
  says, and the result is cropped to the selection. Away from those sizes the model fails: at
  one and two megapixels (1344 × 736, 1824 × 1088) a photographic scene came out smeared and
  waxy, where the same prompt at 2560 × 1440 was sharp (2026-10-12). A render size of 0 still
  renders at the selection's own size. A picture it is given (a fill, a restyle) is worked on
  at the render size in megapixels, 4.2 unasked.

- **Prompt writer** (text to image; its settings' *Prompt writer*, a Gemma 4 text encoder such
  as `gemma4_e4b_it_fp8_scaled`). The model was trained on one descriptive paragraph per
  picture. Given a list of instructions ("Create…", "The man should…", "Avoid: …") it drew
  posed people with waxy skin; given the same request as a description, candid ones with real
  skin. With a writer set, ComfyUI's Generate Text node rewrites the prompt that way before the
  picture is rendered (same seed, same rewriting); the prompt you typed is what the layer
  records. Without one the prompt goes in as typed.
- **Refining pass** (text to image; *Refine with*, Z-Image Turbo with its text encoder
  `qwen_3_4b` and VAE `ae`). The decoded picture is gone over once by Z-Image Turbo (8 steps,
  CFG 1, `res_multistep`) at **Refine strength** 0.35, which adds the fine texture of skin,
  stubble and fabric and keeps the picture. Leave *Refine with* empty for none.
- **Scan Server** sets both when the server has the files. With both, the coffee-shop scene
  took about 150 s on an RTX 4090 against 60 s without (three models are loaded in turn); raise
  the timeout in Preferences › Generative AI if yours is lower. Neither is used when the model
  is given a picture.

**Ming Image** (0.1 Design, from inclusionAI) is made for layouts with text: pages, posters,
UI screens, logos. Put the copy you want rendered in quotes. It follows ComfyUI's two Ming
templates, and differs from the other models in four ways:

- **It renders at 2 megapixels** unasked (1408 × 1408 for a square), as ComfyUI's template
  does. At one megapixel it is faster but small type is garbled. At four (2672 × 1504 for 16:9) a
  photographic scene came out waxy and gained a third person the prompt did not ask for, where
  the same seed at two megapixels matched the template's result (2026-10-12). Change **Render
  size (MP)** in its settings.
- **A picture it is given is worked on at 2 megapixels at most** (a change, a restyle, a fill),
  whatever the render size is raised to. The picture doubles what the model attends to: an edit at 4 MP took 45 s against 17 s at 2 MP,
  and one stalled for ten minutes while ComfyUI encoded the picture beside the 18 GB text
  encoder on a 24 GB card. A render size of 0 (the selection's own size) is not capped.
- **Alpha Channel** is on for it, on Create. PhotoCraft puts the model's own phrase for it
  (`RGBA, 4-channel, transparent background`) in front of the prompt, unless the prompt already
  starts with one of the phrases its authors list. It is not dependable: the background came
  out transparent in 14 of 20 tries above one megapixel and in 1 of 7 at 1024 × 1024, depending
  on the prompt and the seed, and otherwise it is opaque white. Generate again when it is. With
  the box off the result is always opaque. An edit comes back opaque whatever it is asked.
- **Sampler** in its settings is for text to image (euler). Given a picture it is sampled with
  the LCM sampler of the edit template (noise 1.02): with euler an edit, a restyle and a fill at
  less than full strength came out speckled. CFG 1 uses no negative prompt, as the templates
  do; above 1 the negative prompt is used.

Not run: the templates' prompt enhancer (Qwen 3.8 27B rewriting the prompt into a layout
caption; write the layout out yourself, section by section, or use a custom workflow) and their
*comfy kitchen attention* node (not available on every GPU; 13 s against 14 s at 2048 × 2048).
Ming Image Design Layer, which takes a design apart into layers, is a different model and is
not detected.

Tried against ComfyUI 0.39 (RTX 4090, models loaded): text to image in 3 s at 1024 × 1024 and
14 s at 2048 × 2048; an edit that recolours two buttons and replaces a headline in 17 s, with
the rest of the page unchanged; a restyle at 60% in 22 s. Ticked **Supports inpainting**, it
replaced a cup with a teapot inside a selection, in the scene's light, at 80% and 100% strength
(two tries, one picture).

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
| Google Gemini | `gemini-nano-banana-2.1` (Nano Banana 2.1), `gemini-3-pro-image-preview` (Nano Banana Pro), `gemini-3.1-flash-image-preview` (Nano Banana 2) | the closest supported aspect ratio at 1K / 2K / 4K |
| xAI | `grok-imagine-image-2.0` (Grok Imagine Image 2.0); quality auto / low / medium | the closest supported aspect ratio at 1K / 2K |
| Meta | `muse-image-1.0` (Muse Image) | the selection's shape (up to 16:1); the model decides the pixels |
| Black Forest Labs | `flux-3-image` (FLUX 3 Image). The model id is the endpoint, so `flux-2-pro`, `flux-2-max`, `flux-kontext-pro` … work too | FLUX 3: the closest supported aspect ratio at 1K / 2K / 4K. Older endpoints: pixels, about 1 MP |

Each provider has one key, on the **API Keys** page (`XAI_API_KEY`, `META_API_KEY` and
`BFL_API_KEY` are read when none is saved). How the three newer ones are called:

- **xAI and Meta** answer one request with the image, as OpenAI does. An edit is sent as JSON
  with the pictures as data URLs (xAI: `image`, or `images` for several; Meta: `images`).
- **Black Forest Labs** takes a job and is asked for it every 1.5 s at the address its answer
  names, until it is ready; the image is then downloaded from a signed address. The key is
  sent only to `https://…bfl.ai` hosts: an answer that names any other address to poll is
  refused, and the download carries no key. A cancel stops the waiting (the API has none).
- None of the three takes a seed, a mask or a transparent background. Ticked **Supports
  inpainting**, they are sent the mask as a second picture with an instruction to change only
  that region, as Gemini is; the layer's mask hides whatever else they changed.
- **Cost.** xAI's answer says what the request cost and Black Forest Labs' how many credits
  it used (one cent each); the panel shows that as *Last image $…* under Generate. Muse Image
  is a flat $0.01 an image, shown as *Cost $0.010*.

These were added from the providers' documentation on 2026-10-13 and then run against the live
APIs from the app (text to image, one model each). They are among the recommended models.

**Add Model** offers all of these. GPT Image 2 stays on offer beside 2.5 for those who prefer the
look of its pictures. Nano Banana 2.1 (released 2026-10-06) is offered but not among the
recommended models: it has not been through the comparison, and Google documents it for the
Interactions API only. PhotoCraft calls every Gemini model through `generateContent`, which
Google says stays supported; if 2.1 answers that it is not available there, the error says so
and the model needs an Interactions request added to `cloud.rs`.

Not offered: Midjourney (no public API), Microsoft's MAI-Image (each user would need an Azure
Foundry deployment and its endpoint, not just a key), and Seedream and Qwen-Image 3.0 (possible
later: ByteDance's and Alibaba's APIs take a key, the latter one per region).

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
reports only whether one is set). Without a stored key the provider's environment variable is used
(`OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`, `META_API_KEY`, `BFL_API_KEY`). A key is sent only to its provider. The file is plain text;
protect it like any other credential on the machine.

## For agents

Everything is a command (`photocraft-cli commands | grep '^ai\.'`):

| Command | |
|---|---|
| `ai.generate {"prompt", "model"?, "alpha"?, "seed"?, "rect"?, "mask"?}` | generate onto a new layer; a background job |
| `ai.generate {"prompt", "mode": "inpaint", "strength"?, "context"?, …}` | repaint the selected part of the active pixel layer onto a new masked layer; needs a selection and pixels under it. `cargo run -p photocraft-engine --example ai_inpaint` runs it (and, with `img2img` in place of the rectangle, image to image) against a real server without the app |
| `ai.generate {"prompt", "mode": "img2img", "source"?: {"layer": id} or {"document": id}, "strength"?, …}` | redraw a layer in place, or another open document fitted to the area, onto a new layer |
| `window.panel.aiGen.loadLayer {"layer"?: id}` → `{loaded}` | a shell command (the running app): load what a generated layer was made with into the panel, like clicking it; the active layer without `layer`. `loaded` is false for a layer that was not generated, and while a generation runs |
| `ai.mask {"paint"?: bool, "clear"?: bool}` | paint the area to inpaint, or to scope an edit, instead of selecting it: Quick Mask, started empty, tinted where selected; then `paint.stroke {"target": "quickMask", "color": "#ffffff"}` adds to it. `ai.generate` leaves the mode itself, for `inpaint` and for `imgEdit` |
| `ai.generate {"prompt", "mode": "imgEdit", "source"?, "references"?: [{"layer": id} or {"document": id}, …], "quality"?: "draft"/"standard"/"detail", "context"?, …}` | change a picture following the prompt as an instruction, onto a new layer; with a selection only the part around it is sent, and the layer is masked by it. `--example ai_inpaint` runs it with `edit` or `edit:x,y,w,h` in place of the rectangle |
| `ai.sources {"mode"?: "img2img"/"imgEdit"}` | what img2img (or an edit, and its references) can start from now: the active document's layers with pixels, the other open documents, for an edit also the active document itself, and which is the default |
| `ai.models` | the models, whether each can run now, generates from a prompt (`generate`), is offered for inpainting (`inpaint`), edits (`edit`) and how many `references` an edit with it takes |
| `ai.comfyModels {"url"?}` | the model files a ComfyUI server has |
| `ai.server.status` / `ai.server.find` / `ai.server.stop` | the local server: state, how the running one was started, stop the one PhotoCraft started |
| `ai.setKey {"provider", "key"}` / `ai.keys` | set a key for this session / which providers have one |

`ai.models` returns every model with `listed` (offered in the panel) and, as `listed`, the ids
the panel offers in its order.

Settings live at `integrations.ai` (`prefs.get` / `prefs.set`), whatever the dialog calls the
section; `edit.preferences.generativeAi` returns them, and in the app opens the dialog there. The panel's state is
`ai_gen` in the UI state that `ui.inspect` returns: `mode` (`create` or `edit`), `task` (Edit's:
`change` or `restyle`), `method` (`instruct` or `fill`, for a model that changes a picture both
ways), `model` (Create and Restyle), `editModel` (Change), `strength`, `context`, `maskBrush`,
`imgSource`, `imgStrength`, `editSource`, `editReferences`, `editQuality`, `prompt`, `alpha`,
`seed`, `lastSeed`, `advanced`. The panel's tabs and tasks are the shell's; `ai.generate`'s `mode`
names the technique (`txt2img`, `inpaint`, `img2img`, `imgEdit`), as before.

A generated layer carries its settings as `generation` in the layer tree (`document.inspect`
and the like): `{mode, model, modelName, prompt, strength, context, alpha, seed, source, references, quality}`, named
as `ai.generate` names its params, so they can be passed straight back to it. In the code it is
`photocraft_doc::Layer::generation`.

## Code map

- `crates/aigen` (L4): models and settings, the ComfyUI workflows (`comfy.rs`), cloud requests
  (`cloud.rs`), size rules (`size.rs`), model detection (`detect.rs`), and the one network seam,
  `Transport` (`http.rs`; `UreqTransport` on native, none on wasm). Everything but the transport
  is pure and tested without a network. Try a workflow against a real server with
  `cargo run -p photocraft-aigen --example generate -- http://127.0.0.1:8188`; add
  `inpaint image.png mask.png [strength]` after the output file to try an inpainting.
- `crates/engine/src/ai_cmds.rs`: the commands. The app shell sets `Session::ai.transport`.
- `crates/ui-egui/src/ai_gen_ui.rs`: the panel and the Preferences › Generative AI page.
