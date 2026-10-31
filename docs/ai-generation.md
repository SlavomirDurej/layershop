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
   - **Manage Models**. At the top, on one line, what is set up: **Cloud models** and **Local
     models**, each with its count and a switch that offers that kind in the AI Gen panel or
     leaves it out (what each kind is: in its tooltip). A provider without a key, or a server
     that could not be read, is said on a line under it.
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
   (strength, context, the source picture, alpha), and the seed. Select the layer and click
   **Load from layer** (beside the model; just its icon in a narrow panel) to put the prompt it was made with in
   the prompt box, in place of what is there, and choose the model it was made with again
   (Change's model for a fill or a change, the other one for a creation or a restyle), when
   that model is still offered. The tab, the task and the other settings stay as they are.
   Selecting a layer by itself loads nothing (except on the Regions tab, below). The button is off for a layer
   that was not generated, and while a generation is running. The settings are saved in `.pcraft` files, in PSD and PSB files and in
   layered TIFFs, and copied with the layer (Duplicate Layer); flat formats (PNG, JPEG…) don't
   keep them. In a PSD they are one item (`pcGn`, JSON) of the layer's `shmd` metadata block,
   next to the items Photoshop keeps there: other programs step over it, so the file opens as
   any PSD does. Whether Photoshop writes the item back when it saves the file again has not
   been checked; if it does not, the settings are gone from that copy.

**Size.** The selection decides the shape. Models are trained near one megapixel and need edges
in multiples of 16, so a model renders at about its **Render size (MP)** in the selection's
aspect ratio, and the result is scaled to cover the selection and cropped to it. Set it to 0 to
render at the selection's own size. The panel shows what will be asked (`Selection 520 × 571 px
→ 976 × 1072 px`). Ming Image is an exception: it renders at two megapixels (see *Ming Image*
under *Local models*). MOSS Image is the other: it renders at the selection's own size (see
*MOSS Image*).

## Create and Edit

The two tabs across the top of the panel say what you are there to do.

**Create** generates a new image from the prompt alone, as above. It lists the models that
generate. Its second tab, **Regions**, lays the scene out in boxes for the models trained on
that (see *Regions*).

**Edit** changes a picture that is there. The row under the tabs chooses the task:

- **Change** changes the picture as the prompt says. With a selection only that part changes, and
  the rest stays exactly as it was; without one the whole picture is changed. It lists the models
  that edit or inpaint, and how the model is asked follows from the model:
  - an **edit model** (Qwen Image 2.1, FIBO 2, Ming Image, Qwen Image Edit, FLUX.2, the cloud models) is shown the
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
share theirs. Until one is chosen, Change uses Qwen Image 2.1, the recommended edit model, when it is
set up, ahead of the cloud models (so does `ai.generate` with `"mode": "imgEdit"` and no `model`).
FLUX.3 Klein is not offered for Change: ComfyUI's template edits with it, but asked
to add a deer to a meadow it drew malformed animals (2026-10-16), so it creates and restyles only.

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

## Regions (Create › Regions)

Some models were trained on prompts that place what they describe in boxes. For those, Create
has a second tab beside **Prompt**: **Regions**. It lays the scene out in boxes ("regions"),
each with what goes in it, and the model is told both.

- **Which models.** Only models trained on boxes: **MOSS Image**, **Ideogram 4**, **FLUX.3
  Klein**, **FLUX 3 Image** (Black Forest Labs' API) and **FIBO 2**. For any other model the
  tab is off, and its tooltip names the ones that can. FLUX.2, Qwen Image, GPT Image, Nano
  Banana and the like read a layout as words ("on the left"); they get the Prompt tab only. The
  choice of tab is kept: pick a model that takes regions and they are back as they were.
- **On the canvas.** While the tab is in front the boxes are drawn over the picture: an outline in
  the region's colour, its fill, a label with its depth and type, and its description. Opening
  the tab gives the pointer to them (it picks the Move tool): click a box or its label to
  select it (the label reaches a box behind others; labels are drawn over every box), drag a
  label or the selected box to move it, and the selected box's corners to resize it. A drag anywhere else draws a new
  box (shown as you drag, in the colour it will get, with its size), over other boxes too, so
  a canvas they already cover still takes one. A new box opens
  for typing at once; double-click a box to type its description again (Esc or Ctrl+Enter
  ends). Picking any other tool hands the pointer back to it; the boxes stay in view until the
  tab is left.
- **In the panel.** **Scene** is the prompt: the whole picture, its setting, light and style.
  Under it, **Regions (N)**: **?** says how it works, then copy, clear all and add. Each row
  is a region, front to back: drag a row up or down to change its depth (the top row is in
  front), the pencil (or a double-click on the row) opens its **Prompt** (what goes in the
  box), **Type** (Object, Character, Text), the **Words** of a text
  region, its **Colour** and its **Fill** and fill opacity, one per line under the row (it
  closes when another region is selected or on a click outside it), and the bin removes it.
- **Copy** puts on the clipboard exactly what Generate would send the chosen model: its layout
  format, for the selection's bounds (or the canvas).
- **Kept with the document.** The regions are saved in `.pcraft` files and in PSD and PSB files
  (an image resource of PhotoCraft's own, 4001 `pcRG`, beside the references' 4000, which other
  programs step over), and every change is a History step. A layer generated from them keeps
  them too: **Load from layer** on it puts the scene, the regions and the model back, with the
  Regions tab.
- **The selected layer's layout.** With a layer made with regions selected, the Regions tab
  shows its scene and regions (on opening the tab, and on selecting another such layer while
  it is open), and changes to them, the scene typed included, are kept in that layer in the
  same History step: Load from layer then brings back the layout as last edited, not the one
  the pixels were made from. With any other layer selected the regions are the document's
  alone.
- **The area.** Boxes are in document pixels. A generation is made into the selection's bounds
  (or the canvas): the boxes are taken relative to that area, a box is cut where it leaves it,
  and one wholly outside it is left out.

What each model is sent (every format is the model's prompt, so it goes the way any prompt
goes). No format has a depth field: regions are written back to front, so what is in front
comes last, as the vendors advise for overlaps; saying how they overlap in the descriptions
("asleep on the armchair cushion") helps more.

| Model | Format | Boxes |
|---|---|---|
| MOSS Image | its authors' layout JSON: `summary.caption` and `items` labelled `object`, `character` or `text` (with the words in `text`) | `[top, left, bottom, right]` in pixels from the area's centre (`abs_centralized`, `yxyx`), scaled with the render size |
| Ideogram 4 | its structured caption: `high_level_description` and `compositional_deconstruction` with an empty `background` and `elements` of type `obj` (a character too) or `text` | `[top, left, bottom, right]` on 0–1000 from the top left |
| FLUX 3 Image, FLUX.3 Klein | the scene citing each box as `<obj_N>`, a space, then a JSON array of `{"id", "bbox", "desc"}` rows; text quoted in `desc` (`text reading "…"`) | as Ideogram's |
| FIBO 2 | Bria's VGL: `short_description`, `objects` and `text_render` | `[x, y, width, height]`, 0–1 from the top left |

Not yet tried against the models themselves (ComfyUI was not installed when this was built,
2026-10-30). Each format follows its vendor's documentation, and these are still to check with
a render:

- FIBO 2's VGL documents normalised boxes but neither their range nor their origin.
- ComfyUI's own Ideogram 4 layout node builds a dictionary, not text; whether the same JSON as
  text through the text encoder is followed as well is to be seen.
- FLUX.3 Klein runs here without its prompt upsampler, which Black Forest Labs say layouts are
  weaker without.
- Whether MOSS Image draws later items over earlier ones.

All of them treat boxes as guidance, not as clipping: objects spill past their boxes, and very
small boxes may be dropped.

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
- The models offered are the ones that edit: **Qwen Image 2.1**, **FIBO 2**, **Ming Image** and
  **FLUX.2** (which also generate), **Qwen Image Edit** 2509 / 2511 (which do nothing else; Scan Server adds them, with
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

## Setting up local AI (the wizard)

**File › Set Up Local AI…** installs everything local models need, for someone who has never
heard of ComfyUI: a private copy of ComfyUI with its own Python, and the model files. No
terminal, no Python install, nothing added to `PATH`, no administrator rights. Desktop only, and
installing the engine is Windows only for now ("Use my ComfyUI" works on any desktop).

It opens from four places: the File menu; **Preferences › Generative AI › Local Server**
(**Set Up Local AI…**, or **Add Models…**, **Repair** and **Remove Local AI…** once set up;
the dialog is saved first); the line under Generate in the AI Gen panel, until local AI is set
up; and by itself, once, on the first launch after installing (`--setup-ai` on the command line
opens it at any launch; an app started with `--control` is not asked). It is a window, not a
dialog: the editor stays usable beside it.

| Step | Shows | Changes anything? |
|---|---|---|
| 1. Your PC | Windows version, graphics card and its memory, driver, free space, an existing ComfyUI, whether the download servers answer | No |
| 2. Location | The engine folder (default `%LOCALAPPDATA%\LayerShop\engine`) and the models folder (default beside it; any drive), with the room needed against the room free | No |
| 3. Models | Light / Starter / Recommended / Everything (Light, MOSS Image alone, on a card under 16 GB, its model listed first and "suggested for your card"), and a checklist: size, what each is for, its licence and whether that allows commercial work, a gated model's two steps (Sign in with Hugging Face, Accept the licence), "not tested on cards this size" | No |
| 4. Install | One row per file with its progress; overall size, speed and time left; Pause | Yes |
| 5. Ready | The test image, how long it took, how many models the panel lists | Starts the engine |

- **No graphics card the engine can use** (a virtual machine's display adapter, Windows' basic
  one, or none) stops step 1: the card row says local models can't run here and points to cloud
  models, and Next stays off. Ticking **Install anyway, to run on the processor** under that row
  installs the engine anyway and starts it with ComfyUI's `--cpu` (also in the settings it
  leaves); the last step then only checks that the engine starts, with no test image, as these
  models take an hour or more per image on a processor if they fit in memory at all. For trying
  the setup out in a VM. (`ai.setup.probe` says `gpuUsable`; `ai.setup.plan`/`install` take
  `cpu`.)
- **When the engine dies while starting**, the error gives Windows' exit code in words (access
  violation, an instruction the processor lacks, a missing DLL), Python's crash-dump line and the
  call it crashed in (not the dump's last, outermost lines), and where its whole output is
  (`%APPDATA%\Photocraft\comfyui.log`).
- **The engine's step says what the engine is doing**, read from what it prints
  (`aigen::activity`): "Starting ComfyUI: loading PyTorch · 3:12", then for the test image
  "Generating with …: loading QwenImage into memory", "step 7 of 20, about 03:50 left", with a
  clock that keeps moving. **What's happening?** beside it opens the engine's own last 40 lines,
  live, and where all of it is (`ai.setup.status` › `engine`). A start that hangs ends after 10
  minutes of real time (a processor-only VM can take minutes to load PyTorch).
- **It carries on with the window closed.** Close (or the window's ×) while it runs asks
  **Keep Downloading** (the window closes, the setup goes on) or **Stop Download** (it pauses;
  what arrived is kept). The status bar shows how far it is, and clicking it (the bar or its
  words) brings the wizard back on the Install step; so does the line under Generate.
- **It can be stopped at any point**, by Pause, by closing the app or by a crash. What arrived is
  kept: files are downloaded to `<name>.part` and continued from where they stopped; the next
  launch offers **Continue Setup**. There is no separate resume code: a setup is always "plan
  what is missing, then run the plan", so resuming, adding a model later and repairing are the
  same thing.
- **Every file is checked against a pinned sha256** before it takes its final name. A file that
  arrives wrong is fetched once more, then refused with both checksums in the message.
- **Shared files are downloaded once** (Ideogram 4 uses Qwen Image 2.1's text encoder), and the
  totals count them once: Light 13.0 GB, Starter 18.5 GB, Recommended 44.1 GB, Everything
  190.5 GB. FIBO 2 Dev (13.6 GB alone, 8.4 GB beside FLUX.3 Klein) is offered unticked: only
  Everything has it.
- **A small card starts on Light.** With less than 16 GB of video memory
  (`photocraft_aisetup::manifest::LIGHT_BELOW_VRAM_GB`), the Models step starts on the Light
  preset, MOSS Image (13.0 GB, Apache 2.0, ungated), and lists it first; larger cards and
  unknown ones start on Recommended. A selection the user has already changed is kept when the
  probe answers. `ai.setup.manifest` says `suggested` (`light` | `recommended`) and orders
  `models` to match. "Not tested on cards this size" still shows: MOSS Image has only been run
  on a 24 GB card.
- **Gated models** (FLUX.3 Klein: Black Forest Labs' FLUX Kommunity License) can be
  downloaded only by someone signed in to Hugging Face who accepted the licence on the model's
  page. Its row only says where it stands (**Needs a Hugging Face sign-in (below)**, **Licence
  to accept (below)**, **Ready to download**, **Not available in this version**); what to do is
  in one place, the Hugging Face panel under the list, which stays in view however far the list
  is scrolled: **Sign In with Hugging Face** → **Next: accept the licence of …** with **Accept
  the Licence** (opens the model's page; **Check again**, and coming back to the app checks by
  itself) → signed in, nothing left to do. Install with it ticked and not ready asks first
  (**Install**, **Install Without FLUX.3 Klein**, **Cancel**): it is never silently left out.
  (Until 2026-10-24 the sign-in was a link in the model's row as well as the panel: two places,
  and the row had to be scrolled to.)
  - **Sign in with Hugging Face** is OAuth 2 with PKCE as a public app (no client secret):
    the browser opens Hugging Face's page and comes back to a one-shot listener on
    `127.0.0.1:<any free port>`; a `state` that is not the one sent is refused, and it gives up
    after five minutes (or Cancel). Scopes `openid profile gated-repos`: who the user is, and the
    public gated repos they were granted, nothing private. The access token lasts hours and is
    renewed with the refresh token before it expires (the access check just before Install
    renews it; gated files are downloaded first, while it is fresh). The app's client id is
    `photocraft_aisetup::hf::CLIENT_ID` (`LAYERSHOP_HF_CLIENT_ID` overrides it to try an app);
    empty, Sign in is unavailable and the token is the way in.
  - Where a model stands is one `HEAD` of its gated file with the token: 2xx (or the redirect
    to the CDN) ready, 401 not signed in (Hugging Face answers a token it doesn't take the same
    way, `X-Error-Code: GatedRepo`), 403 signed in without the licence, 404 **unavailable**: the
    maker renamed the file, and only a newer manifest knows where it went (signed out, Hugging
    Face answers 401 before it looks for the file, so this shows only to someone signed in; BFL
    renamed FLUX.3 Klein's files on 2026-10-17 and that is how it showed). Nothing is downloaded.
  - A sign-in Hugging Face no longer takes is not shown as signed in: one that ran out with no
    refresh token to renew it (`Credentials::expired`; `hfStatus` says `expired`), or one a
    `HEAD` answers 401 (it expired, or the app was revoked in Connected apps), is forgotten by
    the access check, and the panel says why above **Sign In with Hugging Face**. The app's
    access tokens last as long as its Hugging Face settings say (30 days at most).
  - **Use an access token instead** (collapsed) is the fallback, for organisation token
    policies or a sign-in that fails: a read token, checked with `/api/whoami-v2`. A token
    already on this PC (`HF_TOKEN`, or `huggingface_hub`'s token file) is offered as **Use the
    sign-in found on this PC**, and used only on that click. Signed in with Hugging Face, the
    token fallback is hidden.
  - The sign-in (access and refresh token, expiry, username) is kept in `ai-keys.json` with the
    API keys: never in the preferences, never in what a command returns or `ui.inspect`
    reports, never logged. Every request with a token goes to `https://huggingface.co/` only;
    the downloads drop it on the CDN redirect. **Sign out** forgets it here; **Connected apps**
    links to Hugging Face's settings, where the app is revoked. **How does this work?** links to
    <https://layershop.aardvarkstudios.net/hugging-face/>.
- **The engine** is ComfyUI's official Windows portable build, pinned by version and sha256
  (0.43.0 today, since 2026-10-30 for FIBO 2; 0.42.1 before, for MOSS Image; NVIDIA, NVIDIA with CUDA 12.6 for older cards and drivers, AMD and Intel
  builds, the last two untested). It is unpacked in pure Rust; the archive is refused whole if
  any entry would land outside the engine folder. It gets a port of its own (8190 or the next
  free one), so it never collides with a ComfyUI on 8188 or ComfyUI Desktop on 8000, and it is
  started hidden when a local model needs it and stopped when the app quits.
- **Use my ComfyUI** installs nothing: it connects to the ComfyUI at an address, reads how it
  was started when it runs on this computer, scans its model files, and offers to download only
  the recommended files it lacks into its models folder. When the first screen finds a ComfyUI
  (running, or installed by ComfyUI Desktop), it asks with two big buttons in place of Next:
  **Continue with My ComfyUI** or **Continue with New ComfyUI** (recommended); without one, Use
  my ComfyUI is a link in the footer. Its screen has **Install New ComfyUI Instead** to change
  one's mind.
- **An older ComfyUI of one's own is told what won't run.** Each model of the setup names the
  first ComfyUI release that runs it (`minComfy` in `crates/aisetup/manifest.json`, read from
  ComfyUI's release tags: the newest of its architecture, its nodes and its weight format):
  Ernie 0.19.0, HiDream O1 0.21.1, Ideogram 4 0.24.0, Z-Image Turbo and Krea 2 0.27.0 (int8
  weights), Qwen Image 2.1 0.37.0, Ming Image 0.38.0, FLUX.3 Klein 0.41.0, MOSS Image 0.42.0,
  FIBO 2 0.43.0. None may be later than the pinned engine (the manifest is refused otherwise).
  The version is what the ComfyUI says when Connect reaches it (`ai.comfyModels` › `version`),
  else, before that, what the first look found (the running one's `/system_stats`, or the
  installed ComfyUI Desktop's `comfyui_version.py`; the first screen's row reads "ComfyUI 0.41.0
  detected"). When it is older than the pinned engine, the Your ComfyUI screen says so under
  the address, names the offered models it can't run with the version each needs, and says
  other features may not work, to update ComfyUI or install a new one instead; the Models page
  says "needs ComfyUI 0.43.0 (yours is 0.41.0)" beside each such model. They can still be
  ticked: ComfyUI can be updated later. A version that can't be read claims nothing.
- **Its models folder is filled in.** The folders a ComfyUI reads models from come from the
  ComfyUI itself when it runs (`GET /experiment/models`), else from how it is started: its base
  folder or `--models-directory`, and the extra model paths files it reads (ComfyUI Desktop
  passes one listing its shared models folder). The first, where ComfyUI saves new models
  itself, is suggested; the others are a click away, and a folder it doesn't read is warned
  about. With none found, the screen says where the folder usually is.
- **Cloud models only** closes the wizard and opens the API Keys page.
- **Remove Local AI…** deletes the engine, and if asked the model files the setup lists, and
  says how much room that freed. Only what the setup installed is touched: files you put in the
  models folder yourself stay, and a ComfyUI you set up yourself is never removed.
- **Uninstalling the app** (the MSI) asks what goes with it: the engine (ComfyUI with its own
  Python; no Python of the system's is ever installed), the models the setup downloaded (both
  ticked), and the settings, presets, API keys and Hugging Face sign-in (not ticked), each with
  its size. Model files in a folder the user chose go only where the setup put them; models
  downloaded into the user's own ComfyUI are never touched. An upgrade asks nothing and keeps
  everything; so does a silent uninstall unless `REMOVEDATA=ai|all`. The code:
  `apps/photocraft/src/uninstall.rs` (`photocraft.exe --uninstall-cleanup`), run by the MSI.

What can go wrong is said where it happens: a drive without the room (found before anything is
downloaded, with the drive and the amount), a folder path too long for Windows, a network that
stays down (the setup pauses; Continue picks up), a gated file (skipped, the rest carries on),
an antivirus holding a file, an engine that does not start (the last lines of `comfyui.log`), a
test image that runs out of memory (not a failed install: a smaller model is suggested).

The setup's own record (what was chosen, how far it got) is `ai-setup.json` in the settings
folder: a file of its own, not a preference.

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
  For the same reason they cannot run the setup wizard's installing commands.

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
| FLUX.3 Klein | local | 50 steps, CFG 3.5, 1 MP |
| MOSS Image | local | 25 steps, CFG 2.5, `er_sde`, the selection's own size |
| FIBO 2 Dev | local | 50 steps, CFG 5, structured negative, 1 MP |
| Ideogram 4 | local | 28 steps, CFG 5, shift 5, 1 MP |
| HiDream O1 | local | 40 steps, CFG 5, `dpmpp_2m_sde_gpu`, noise scale 8, its trained sizes; prompt written by Gemma 4, result refined by Z-Image Turbo at 0.35 |
| Ernie Image Turbo | local | 8 steps, CFG 1, 2 MP |
| Ming Image 0.1 Design | local | 12 steps, CFG 1, 2 MP |

**In the panel** the model list is in two sections, headed **Cloud API** and **Local Models**;
the names are the models' own, and so is the tag in the name of the layer a generation lands on.

**Only models that can run are offered.** A ticked model is in the panel's list when it can run
now: a cloud model once its provider's API key is there; a local one once local AI is ready (set
up by the wizard or the Local Server page, or a ComfyUI has answered) and, when the server has
said which files it has, its file is among them. The panel asks the ComfyUI in the settings for
its files, without starting it (`ai.comfyModels` with `start: false`), when it is first shown,
after a Local AI setup ends, and when the app gets the focus back (at most once a minute); a
server that isn't running is not reported as an error, and what it said last stays. While a
setup runs, local models wait: the panel says "Local AI is being set up", with its progress and
**Show Progress**, and cloud models with a key still work. With nothing that can run it says why
and offers **Set Up Local AI…** and **Add an API Key…** (or **Manage Models…**), then **Run
Setup Wizard Again…**, which opens the wizard on its first screen with the last setup's route and
models forgotten (its folders are kept). `ai.generate`
and `ai.models` are unchanged: an agent can still name any model.

**Leaving a kind out.** Switch off **Cloud models** or **Local models** at the top of Manage Models
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
| MOSS Image (SFT, Base) | diffusion model, text encoder (`moss_image`, Qwen 3.5 9B), the small FLUX.2 VAE | skip layer guidance (layer 22), KSampler; a layout prompt; see below |
| FIBO 2 (Dev, Edit Turbo) | diffusion model, text encoder (`fibo2`, Qwen 3 VL 4B), the FLUX.2 VAE | its own prompt node, KSampler; see below |
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

**MOSS Image** (from OpenMOSS, Apache 2.0) renders text to image from a *layout prompt*: JSON
that describes the whole picture and places what it describes in boxes on the canvas. That suits
a selection: what you ask for is drawn to fill it. It follows ComfyUI's MOSS Image templates
(25 steps, CFG 2.5, `er_sde`, the authors' negative prompt, skip layer guidance on layer 22 over
the first half of the schedule), and differs from the other models in three ways:

- **It renders at the selection's own size** (the whole canvas without one), in multiples of
  16, so the result is not scaled to fit. Its authors give 0.25 to 4 megapixels as the range it
  renders well: a smaller or larger selection is rendered at the nearest end of it, in its
  shape, and scaled. A **Render size (MP)** above 0 in its settings renders at that size instead.
- **A typed prompt becomes a layout**: the prompt as the picture's caption, and one object with
  the same caption in a box 5% in from the canvas's edges. Without the box the subject sat small
  in the middle of a wide selection; with a box at the very edge it filled the selection but was
  cropped by it (a car without its wheels); at 5% it filled it whole (2026-10-18). A scene
  described this way still comes out as a scene.
- **A prompt that is a layout already goes as written**: a JSON object with `summary` or `items`,
  in the authors' format (see [MOSS Image](https://openmoss.ai/MOSS-Image/)):

  ```json
  {"summary": {"caption": "A minimalist travel poster, cream background, a blue mountain"},
   "items": [
     {"label": "text", "caption": "Large dark-blue uppercase title", "text": "EXPLORE", "bbox": [-416, -384, -288, 384]},
     {"label": "object", "caption": "A flat blue mountain illustration", "bbox": [-192, -288, 384, 288]}],
   "bbox_coord_space": "abs_centralized", "bbox_format": "yxyx"}
  ```

  Boxes are `[top, left, bottom, right]` in pixels of the selection (the canvas without one),
  measured from its centre: a 1024 × 1024 selection runs from -512 to 512 each way. Labels are
  `object`, `character` and `text` (with the words to render in `text`). When the render size
  differs from the selection's, the boxes are scaled with it.

It can't change a picture following an instruction or inpaint (the Edit tab doesn't offer it);
Restyle (image to image) gives it the prompt as typed. Not run: the templates' prompt upsampler (the same Qwen 3.5 9B
rewriting the prompt as a caption), which can rewrite a layout away.

MOSS Image came to ComfyUI on 2026-10-16, after its 0.41.0 release, so it needs a later
ComfyUI (ComfyUI Desktop has it); an older server rejects its text encoder type, and the error
says to update. The Local AI setup pins ComfyUI 0.43.0, which has it, and offers it (the Light
preset on a small card); Scan Server finds it on a ComfyUI that has its files (`moss_image_sft_…` or
`moss_image_base_…`, `qwen3.5_9b_w4a8_gptq…`, `flux2-vae_small…`).

**FIBO 2** (from Bria) generates and edits with the same weights, in FLUX.2's latent space,
with Qwen 3 VL 4B reading the prompt. It follows ComfyUI's FIBO 2 Edit templates:

- **Dev** (`fibo2_dev_…`, the full model): 50 steps, CFG 5, `euler` / `simple`, and the
  template's negative prompt, a structured caption of what to steer away from (plastic skin,
  blur, oversaturation).
- **Edit Turbo** (`fibo2_edit_turbo_…`, distilled from Dev, and despite its name a
  text-to-image model too): 4 steps, CFG 1, no negative prompt. Scan Server sets this up from
  the file name.
- **An edit is lined up with the picture pixel for pixel.** Its prompt node scales a single
  picture to the result's size; PhotoCraft asks for the picture's own size, so nothing is
  cropped. With references, each is taken at the size the model was trained at.
- **A picture it is given is worked on at one megapixel at most**, whatever the render size is
  raised to: it was trained there, and above it follows the instruction less (the template's
  notes). A render size of 0 is not capped.
- **Plain instructions work.** The model also reads Bria's structured captions (VGL, JSON), and
  a prompt that is one goes in as written.

Not run: the template's prompt enhancer (Qwen 3 VL 8B rewriting the instruction as a structured
caption, about 3000 tokens with Bria's long system prompt); an instruction as typed was
followed without it. Tried against ComfyUI 0.43.0 (RTX 4090, `fibo2_dev_int8_convrot`): asked
to add a red deer to the meadow picture FLUX.3 Klein failed on, it drew one well-formed stag
and kept the rest of the picture as it was, in 50 s (54 s with the models loading); a red wool
hat put on a rock in 50 s; text to image at 1216 × 832 in 24 s. With a selection it doesn't
know where the selection is, as no edit model does: "add a rowing boat in the grass" with the
bottom-right corner selected drew the boat mid-picture, outside the selection, so nothing
changed; "… in the bottom right of the picture" put it inside, with the rest untouched. In an
outpainting it is shown the whole area, empty margins included, not the picture alone as Qwen
Image 2.1 is: it lines its result up with what it is shown, and the picture alone would be
stretched over the whole. It paints the margins all the same: the meadow shrunk to the middle
of a white canvas, margins selected, "extend the photo outwards" came back with sky, trees and
meadow all round and no seam, in 54 s (one seed).

FIBO 2 came to ComfyUI with its 0.43.0 release: an older server rejects its prompt node, and
the error says to update. The Local AI setup pins 0.43.0 and offers FIBO 2 Dev unticked (only
the Everything preset has it), from Comfy-Org's repack (`Comfy-Org/Fibo-2.0`, which ComfyUI's
template downloads): Bria's own repository holds only the diffusers weights, behind a form. Its
row says "non-commercial use only" and links Bria's page. The weights are licensed CC BY-NC 4.0
(non-commercial use unless you have an agreement with Bria). Scan Server finds it on a ComfyUI
that has its files (`fibo2_…`, `qwen3vl_4b_…`, `flux2-vae…`).

It loads the int8 Qwen 3 VL 4B (`qwen3vl_4b_int8_convrot`, FLUX.3 Klein's), not the template's
bf16 one: the deer and the hat edits came out the same at the same speed (52 s, 50 s), and the
setup then downloads 8.9 GB less.

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
| `window.panel.aiGen.loadPrompt {"layer"?: id}` → `{loaded}` | a shell command (the running app): put the prompt a generated layer was made with into the panel's prompt box, like its Load from layer button (the prompt and the model it was made with, when still offered; a layer laid out in regions brings them back too; tab and settings stay); the active layer without `layer`. `loaded` is false for a layer that was not generated or has no prompt, and while a generation runs |
| `ai.mask {"paint"?: bool, "clear"?: bool}` | paint the area to inpaint, or to scope an edit, instead of selecting it: Quick Mask, started empty, tinted where selected; then `paint.stroke {"target": "quickMask", "color": "#ffffff"}` adds to it. `ai.generate` leaves the mode itself, for `inpaint` and for `imgEdit` |
| `ai.generate {"prompt", "mode": "imgEdit", "source"?, "references"?: [{"layer": id} or {"document": id}, …], "quality"?: "draft"/"standard"/"detail", "context"?, …}` | change a picture following the prompt as an instruction, onto a new layer; with a selection only the part around it is sent, and the layer is masked by it. `--example ai_inpaint` runs it with `edit` or `edit:x,y,w,h` in place of the rectangle |
| `ai.sources {"mode"?: "img2img"/"imgEdit"}` | what img2img (or an edit, and its references) can start from now: the active document's layers with pixels, the other open documents, for an edit also the active document itself, and which is the default |
| `ai.models` | the models, whether each can run now, generates from a prompt (`generate`), is offered for inpainting (`inpaint`), edits (`edit`) and how many `references` an edit with it takes, and the layout format of a model trained on boxes (`regions`: `moss`, `ideogram`, `flux3`, `fibo`; null for the others) |
| `ai.regions` / `ai.setRegions {"regions": [{"id"?, "kind"?, "caption"?, "text"?, "rect"?: [x, y, w, h], "color"?, "fill"?, "fillOpacity"?}, …]}` | the active document's regions, front to back / replace them (an entry with an id changes that region, one without is new and needs a `rect`; order is depth). One History step |
| `ai.region.set {"id", …fields}` | change one region; `"coalesce"` merges a run (typing) into one step |
| `ai.layoutPrompt {"prompt", "model"?, "regions"?, "rect"?}` → `{text, format, area}` | what `ai.generate {"regions": true}` would send that model: the panel's copy button |
| `ai.setLayerPrompt {"layer", "prompt", "coalesce"?}` → `{layer, prompt}` | change the prompt an AI layer keeps (what Load from layer brings back); refused for a layer not made with AI Gen. `ai.setRegions` and `ai.region.set` take `"layer"` too: that AI layer gets the document's regions in the same History step |
| `ai.generate {"prompt", "regions": true or [...], …}` | a text to image laid out in the document's regions (or those given), with a model trained on boxes; `modelPrompt` in the result is the layout sent |
| `ai.comfyModels {"url"?}` | the model files a ComfyUI server has |
| `ai.server.status` / `ai.server.find` / `ai.server.stop` | the local server: state, how the running one was started, stop the one PhotoCraft started |
| `ai.setKey {"provider", "key"}` / `ai.keys` | set a key for this session / which providers have one |
| `ai.setup.probe` | what this machine has for local models: system, graphics card and memory, drives, an existing ComfyUI, whether the download hosts answer; and the rows the wizard's first screen shows. Changes nothing; a background job |
| `ai.setup.manifest {"vramGb"?}` | what the setup can install: models with their files, sizes, licences, whether each is installed and how it suits a card of that size; presets with shared files counted once |
| `ai.setup.plan {"models"?, "route"?, "variant"?, "recheck"?}` | what a setup would download and the room it needs, given what is on disk. Changes nothing |
| `ai.setup.status` | the setup that is running (steps, bytes, speed, time left) or how the last one ended; the setup's record; what is installed now |
| `ai.setup.hfStatus` | the Hugging Face sign-in: signed in or not, as whom, how (`oauth`, `token`), whether Sign in is available and a token is on this PC. Never a token |
| `ai.setup.hfAccess {"models"?}` | where each gated model stands (`signIn`, `licence`, `ready`): one `HEAD` of its gated file. Renews a sign-in about to expire. A background job |
| `ai.setup.start`, `ai.setup.cancel`, `ai.setup.repair`, `ai.setup.remove`, `ai.setup.hfSignIn`, `ai.setup.hfSignOut`, `ai.setup.setToken`, `ai.setup.hfUseFound` | install, stop, fix, delete, sign in and out of Hugging Face, an access token, the token found on this PC. **Refused to agents**: they download and start a program, delete files, or sign in, and are the user's to do from the wizard. So is `ai.setup.plan` with a folder or a server listing of the agent's choosing |
| `ai.setup.open {"page"?, "route"?, "open"?}` | a shell command (the running app): open the wizard, on a screen (`pc`, `location`, `models`, `install`, `ready`), or close it |

`ai.models` returns every model with `listed` (offered in the panel) and, as `listed`, the ids
the panel offers in its order.

Settings live at `integrations.ai` (`prefs.get` / `prefs.set`), whatever the dialog calls the
section; `edit.preferences.generativeAi` returns them, and in the app opens the dialog there. The panel's state is
`ai_gen` in the UI state that `ui.inspect` returns: `mode` (`create` or `edit`), `task` (Edit's:
`change` or `restyle`), `method` (`instruct` or `fill`, for a model that changes a picture both
ways), `model` (Create and Restyle), `editModel` (Change), `strength`, `context`, `maskBrush`,
`imgSource`, `imgStrength`, `editSource`, `editReferences`, `editQuality`, `prompt`, `alpha`,
`seed`, `lastSeed`, `advanced`, `createTask` (Create's: `prompt` or `regions`), `region` (the
region selected). The panel's tabs and tasks are the shell's; `ai.generate`'s `mode`
names the technique (`txt2img`, `inpaint`, `img2img`, `imgEdit`), as before.

A generated layer carries its settings as `generation` in the layer tree (`document.inspect`
and the like): `{mode, model, modelName, prompt, strength, context, alpha, seed, source, references, quality}`
(and `regions`, for one laid out in regions), named
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
- `crates/aisetup` (L4, on top of `aigen`): the Local AI setup. `manifest.json` and
  `manifest.rs` (what can be installed: the pinned ComfyUI builds and the model files, each
  listed once), `probe.rs` (what the machine has), `plan.rs` (a pure function from manifest,
  choices and disk to the steps; every step knows whether it is done), `fetch.rs` (the streaming
  network seam and its redirect policy; `UreqFetch` on native), `download.rs` (one file: resume,
  sha256, retries), `unpack.rs` (7z with the path and size guards), `runtime.rs` (the engine's
  launch line, model paths, removal), `configure.rs` (the settings a setup leaves), `install.rs`
  (the worker that runs a plan, two downloads at a time), `state.rs` (the record that makes
  a setup resumable) and `hf.rs` (Hugging Face for the gated models: Sign in with PKCE and its
  loopback listener, refresh, userinfo / whoami, the access check, the token found on this PC). Everything but `UreqFetch` and the probe's system calls is tested without
  a network: the tests run whole setups against a fake network and engine.
  `cargo xtask ai-manifest` compares the manifest with what GitHub and Hugging Face serve now
  (a gated file through Hugging Face's tree API: its path and size, the sha256 being shown only
  to someone signed in), so a renamed or moved file fails it;
  `cargo run -p photocraft-aisetup --example archive_contents -- <build.7z>` measures a new
  ComfyUI build (unpacked size, longest path) from its header alone.
  `cargo run --release -p photocraft-engine --example ai_setup -- <folder> install [model-id ...]`
  runs a real setup without the app (the same `ai.setup.*` commands, its own folders, the app's
  settings untouched); run again, it continues; `remove` deletes what it installed.
- `crates/engine/src/ai_setup_cmds.rs`: the `ai.setup.*` commands. The app shell sets
  `Session::ai_setup.env` (the downloader, the default folders, the record's file). A setup is
  not a session job (those are cancelled by Esc): it has a worker of its own, and `poll` applies
  its result to the preferences as they are when it ends.
- `crates/ui-egui/src/ai_setup_ui.rs`: the wizard window, its state (`ai_setup` in the UI state
  that `ui.inspect` returns) and its ways in. `ai_setup_ui::demo` is a stand-in network and
  engine: `cargo run -p photocraft-ui-egui --example snapshot -- --ai-setup-demo
  pc|location|models|install|ready|env` draws any screen offscreen without downloading anything
  (also `models-hf`, `models-licence`, `models-ready`, `sign-in`, `sign-in-licence`, `close`).
