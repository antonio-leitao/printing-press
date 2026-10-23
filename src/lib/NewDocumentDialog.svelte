<script lang="ts">
  /* Starting a document, which is the one thing Press does that writes source
     into a folder of the reader's. It may begin blank or as a copy of one of the
     reader's templates.

     This asks the questions and the page acts on the answer: what a new
     document needs once it exists — a place in the library, a build, an editor
     — is the page's to give it. */

  import { onDestroy } from 'svelte';
  import { FolderOpen, Trash2, ZoomIn } from '@lucide/svelte';
  import { open } from '@tauri-apps/plugin-dialog';
  import TemplateCover from '$lib/TemplateCover.svelte';
  import { api } from '$lib/api';
  import { fail } from '$lib/messages.svelte';
  import { modal } from '$lib/modal';
  import { PageVisibilityTracker } from '$lib/pdf-visibility';
  import { theme } from '$lib/theme.svelte';
  import type { DocumentKind, MenuItem, PageSize, TemplateSummary } from '$lib/types';

  /** What the reader asked for, ready to be made. */
  export type NewDocument =
    | { source: 'blank'; kind: DocumentKind; folder: string; name: string }
    | { source: 'template'; template: TemplateSummary; folder: string; name: string };

  let {
    templates: shelf,
    folder = $bindable(null),
    busy = false,
    onclose,
    oncreate,
    onmenu
  }: {
    /** Every template, fetched as the gallery was opened. */
    templates: TemplateSummary[];
    /// Remembered between documents by whoever opens this. Nobody keeps their
    /// papers somewhere new each time, and the folder is the one part of this
    /// that is usually the same.
    folder?: string | null;
    busy?: boolean;
    onclose: () => void;
    oncreate: (request: NewDocument) => void;
    /** A right-click on a card, with what the menu should offer. */
    onmenu: (event: MouseEvent, items: MenuItem[]) => void;
  } = $props();

  type CreationSource = 'blank' | 'template';

  /** The cell a template's page stands in. The shelf's card, because choosing a
      template is the same act as choosing off the shelf: you are looking at a
      page and deciding whether it is the one. */
  const TEMPLATE_PAGE_WIDTH = 150;
  const TEMPLATE_PAGE_HEIGHT = 194;

  /** What a closer look leaves around the pages, and the room the name under
      the last of them takes. The rest of the window is the page: the whole
      point of opening one is that the card was too small to read.

      `.look-column`'s padding is this number — a page is sized to leave the
      margin, and the column has to actually leave it. */
  const LOOK_MARGIN = 56;
  const LOOK_CAPTION = 40;

  /** One scale for every page of a template, the largest at which none of them
      overflows the window.

      One rather than a fit per page, because these are the pages of a document
      and a document is read at one size — the reader does the same. Taking the
      smallest fit is what keeps a landscape page in an upright document from
      running off the sides. */
  function lookScale(pages: PageSize[], box: number, tall: number) {
    return pages.reduce(
      (smallest, page) => Math.min(smallest, box / page.width, tall / page.height),
      Number.POSITIVE_INFINITY
    );
  }

  /** A page's size once fitted into a cell.

      The card draws its cover into a fixed cell and has to know where the
      paper's edges landed: the ring goes round them, and the magnifier sits on
      a corner. Fitted rather than stretched — a template changes the paper, and
      a beamer slide is not the shape of a pocket book. A closer look fits a
      whole document rather than one page, which is `lookScale`. */
  function fitPage(cover: { width: number; height: number }, box: number, tall: number) {
    const scale = Math.min(box / cover.width, tall / cover.height);
    return { width: cover.width * scale, height: cover.height * scale };
  }

  /// Removing a template from the shelf changes this copy of it; the list the
  /// gallery was opened with is fetched again next time.
  // svelte-ignore state_referenced_locally
  let templates = $state<TemplateSummary[]>(shelf);
  let creationSource = $state<CreationSource>('blank');
  let blankKind = $state<DocumentKind>('latex');
  let templatePick = $state<number | null>(null);
  let newName = $state('');
  /// Whether the chosen template's pages are being looked at closely. The
  /// gallery stays open underneath: this is a closer look at the thing being
  /// chosen, not a place of its own, and it is dismissed the way it was asked
  /// for.
  let looking = $state(false);
  /// The pages of the template being looked at. Starts as the one page the card
  /// already knew about, so the look opens on a page rather than on nothing,
  /// and grows into the whole document when the layout arrives.
  let lookPages = $state<PageSize[]>([]);
  /// Shared observers for that column, so a template of two hundred pages draws
  /// the two on screen rather than all of them. Made when the look opens and
  /// disconnected when it closes: it belongs to one column, not to the page.
  let lookTracker = $state<PageVisibilityTracker | null>(null);
  /// The window, so a closer look can be as large as the window allows and
  /// stay that way when the window changes.
  let windowWidth = $state(1280);
  let windowHeight = $state(800);

  const dark = $derived(theme.isDark);
  const picked = $derived(templates.find((one) => one.id === templatePick) ?? null);
  const newExtension = $derived(
    creationSource === 'blank'
      ? blankKind === 'markdown'
        ? 'md'
        : 'tex'
      : (picked?.entry.split('.').pop() ?? '')
  );

  // Whatever is still observing when the gallery goes, with or without a look
  // open in it.
  onDestroy(() => lookTracker?.disconnect());

  function closeGallery() {
    closeLook();
    onclose();
  }

  function chooseCreationSource(source: CreationSource) {
    creationSource = source;
    if (source === 'blank') closeLook();
  }

  /// Space on a card is the closer look — Quick Look's key, for the same act.
  /// On the card rather than on the dialog, because everywhere else in here
  /// Space is what it always is: the name field takes a space, and Create takes
  /// a press.
  function cardKeys(event: KeyboardEvent, template: TemplateSummary) {
    if (event.key !== ' ' && event.code !== 'Space') return;
    if (!template.cover) return;
    // Or the card would be pressed as well, this being a button.
    event.preventDefault();
    look(template);
  }

  /// The chosen template, opened large. Choosing it is part of asking: what is
  /// being looked at closely is what Create would make, and there is no second
  /// kind of chosen.
  function look(template: TemplateSummary) {
    if (!template.cover) return;
    templatePick = template.id;
    lookPages = [template.cover];
    lookTracker?.disconnect();
    lookTracker = new PageVisibilityTracker(null);
    looking = true;
    void loadLookPages(template);
  }

  function closeLook() {
    looking = false;
    lookPages = [];
    lookTracker?.disconnect();
    lookTracker = null;
  }

  /// The rest of the document behind the cover.
  ///
  /// Asked for once a look is open rather than with the gallery: it is a read
  /// of a PDF per template, and a shelf of them would pay for every template to
  /// answer a question about one.
  async function loadLookPages(template: TemplateSummary) {
    try {
      const pages = await api.templateLayout(template.id);
      // Only if this is still what is being looked at — the layout is a round
      // trip, and a second one can have been opened in the meantime.
      if (looking && templatePick === template.id && pages.length > 0) lookPages = pages;
    } catch {
      // The cover is already up and is what the card showed. A template whose
      // kept PDF cannot be read past its first page is not worth interrupting
      // the choice for.
    }
  }

  async function chooseFolder() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: 'Choose a folder',
        defaultPath: folder ?? undefined
      });
      if (typeof selected === 'string') folder = selected;
    } catch (reason) {
      fail(reason);
    }
  }

  /// Hands the answer over, once there is a whole one.
  function createDocument() {
    const name = newName.trim();
    if (!folder || !name) return;
    if (creationSource === 'blank') {
      oncreate({ source: 'blank', kind: blankKind, folder, name });
    } else if (picked) {
      oncreate({ source: 'template', template: picked, folder, name });
    }
  }

  function templateMenu(template: TemplateSummary): MenuItem[] {
    return [
      {
        label: 'Remove…',
        icon: Trash2,
        danger: true,
        run: () => void removeTemplate(template)
      }
    ];
  }

  /// No confirmation. A template is a copy of something that still exists, so
  /// removing one loses nothing that cannot be saved again from the document it
  /// came from — which is not true of a project, and is why that one asks.
  async function removeTemplate(template: TemplateSummary) {
    try {
      await api.deleteTemplate(template.id);
      templates = templates.filter((one) => one.id !== template.id);
      if (templatePick === template.id) {
        templatePick = null;
        closeLook();
      }
    } catch (reason) {
      fail(reason);
    }
  }
</script>

<svelte:window bind:innerWidth={windowWidth} bind:innerHeight={windowHeight} />

<!-- Wider than the rest so the template branch has room to compare pages.
     Still a dialog: starting a document is one act with a few questions in it,
     not a place of its own. -->
<dialog class="gallery" use:modal={closeGallery} aria-labelledby="gallery-title">
  <h2 id="gallery-title">New document</h2>

  <div class="scopes creation-options" role="radiogroup" aria-label="How to begin">
    <button
      class="scope"
      role="radio"
      aria-checked={creationSource === 'blank'}
      onclick={() => chooseCreationSource('blank')}
    >
      <strong>Blank document</strong>
      <span class="quiet">Start with a new Markdown or LaTeX file.</span>
    </button>
    <button
      class="scope"
      role="radio"
      aria-checked={creationSource === 'template'}
      onclick={() => chooseCreationSource('template')}
    >
      <strong>From a template</strong>
      <span class="quiet">
        {templates.length === 0
          ? 'No saved templates yet.'
          : `${templates.length} saved ${templates.length === 1 ? 'template' : 'templates'}.`}
      </span>
    </button>
  </div>

  {#if creationSource === 'blank'}
    <h3>Format</h3>
    <div class="scopes creation-options" role="radiogroup" aria-label="Document format">
      <button
        class="scope"
        role="radio"
        aria-checked={blankKind === 'markdown'}
        onclick={() => (blankKind = 'markdown')}
      >
        <strong>Markdown</strong>
        <span class="quiet">An empty <code>.md</code> file.</span>
      </button>
      <button
        class="scope"
        role="radio"
        aria-checked={blankKind === 'latex'}
        onclick={() => (blankKind = 'latex')}
      >
        <strong>LaTeX</strong>
        <span class="quiet">A minimal article with a blank page.</span>
      </button>
    </div>
  {:else}
    <h3>Templates</h3>
    {#if templates.length === 0}
      <p class="quiet gallery-empty">
        Right-click a document in the library and choose <strong>Save as template</strong>. Press
        keeps a copy without changing the original.
      </p>
    {:else}
      <div class="gallery-shelf">
        <div class="template-grid">
          {#each templates as template (template.id)}
            {@const paper =
              template.cover
                ? fitPage(template.cover, TEMPLATE_PAGE_WIDTH, TEMPLATE_PAGE_HEIGHT)
                : null}
            <article class="template-card" class:chosen={templatePick === template.id}>
              <button
                class="template-open"
                onclick={() => (templatePick = template.id)}
                ondblclick={createDocument}
                onkeydown={(event) => cardKeys(event, template)}
                oncontextmenu={(event) => onmenu(event, templateMenu(template))}
                aria-pressed={templatePick === template.id}
              >
                <span class="template-page">
                  {#if template.cover && paper}
                    <TemplateCover
                      id={template.id}
                      cover={template.cover}
                      width={paper.width}
                      height={paper.height}
                      invert={dark}
                    />
                  {:else}
                    <span class="template-blank">No page yet</span>
                  {/if}
                </span>
                <span class="template-lines">
                  <strong class="name">{template.name}</strong>
                  <span class="template-files">
                    {template.kind === 'markdown' ? 'Markdown' : 'LaTeX'}{template.fileCount > 1
                      ? ` · ${template.fileCount} files`
                      : ''}
                  </span>
                </span>
              </button>

              {#if paper}
                <button
                  class="template-look"
                  style="top: {TEMPLATE_PAGE_HEIGHT - paper.height + 6}px; right: {(TEMPLATE_PAGE_WIDTH - paper.width) / 2 + 6}px"
                  tabindex="-1"
                  onclick={() => look(template)}
                  title="Look closer"
                  aria-label="Look closer at {template.name}"
                >
                  <ZoomIn size={13} strokeWidth={2} aria-hidden="true" />
                </button>
              {/if}
            </article>
          {/each}
        </div>
      </div>
    {/if}
  {/if}

  <div class="create-row">
    <label class="create-field">
      Name
      <span class="create-name">
        <input
          bind:value={newName}
          placeholder="untitled"
          spellcheck="false"
          autocapitalize="off"
          autocorrect="off"
          onkeydown={(event) => {
            if (event.key === 'Enter') createDocument();
          }}
        />
        {#if newExtension}<span class="create-extension">.{newExtension}</span>{/if}
      </span>
    </label>

    <label class="create-field folder">
      Folder
      <button class="create-folder" onclick={chooseFolder} title={folder ?? ''}>
        <FolderOpen size={14} strokeWidth={1.75} aria-hidden="true" />
        <span>{folder ?? 'Choose…'}</span>
      </button>
    </label>
  </div>

  <div class="dialog-actions">
    <button onclick={closeGallery} disabled={busy}>Cancel</button>
    <button
      onclick={createDocument}
      disabled={busy || !folder || !newName.trim() || (creationSource === 'template' && !picked)}
    >
      {busy ? 'Creating…' : 'Create'}
    </button>
  </div>
</dialog>

{#if looking && picked && lookPages.length > 0}
  <!-- A closer look at the chosen template: its pages, at the size the window
       allows, in a column that scrolls — the same document the reader would
       show, read the same way.

       Its own modal dialog rather than a layer inside the gallery, and for one
       reason: only the top layer can cover a dialog, and what is wanted here is
       a page larger than the dialog the choice is being made in. The gallery
       stays open and untouched underneath, so this adds a step to the act
       rather than a place to be — Escape, Space or a click puts it back.

       A sibling of the gallery in the markup and not a child of it, so that the
       Escape closing this one never reaches the one behind. -->
  {@const zoom = lookScale(
    lookPages,
    windowWidth - LOOK_MARGIN * 2,
    windowHeight - LOOK_MARGIN * 2 - LOOK_CAPTION
  )}
  <dialog class="look" use:modal={closeLook} aria-label={picked.name}>
    <!-- The window is the button: anywhere off the pages dismisses, and so do
         the pages, because there is nothing else to do here. One control, so
         Enter and Space land on it without being asked to — and Space is what
         closes a Quick Look, not what pages it. The column inside is a span
         rather than a div because a button may only hold phrasing content;
         scrolling it works the same. -->
    <button class="look-away" onclick={closeLook} aria-label="Close">
      <span class="look-scroll">
        <span class="look-column">
          {#each lookPages as size, index (index)}
            <TemplateCover
              id={picked.id}
              page={index}
              cover={size}
              width={size.width * zoom}
              height={size.height * zoom}
              invert={dark}
              tracker={lookTracker}
            />
          {/each}
          <!-- After the last page, the way a colophon is. Which template this
               is, and how much of it there was — the count is only worth saying
               once there is more than the one page everybody sees. -->
          <span class="look-name">
            {picked.name}{lookPages.length > 1 ? ` · ${lookPages.length} pages` : ''}
          </span>
        </span>
      </span>
    </button>
  </dialog>
{/if}

<style>
  .quiet {
    color: var(--ink-3);
  }

  /* The library card's name, which a template's caption is set in. */
  .name {
    display: -webkit-box;
    overflow: hidden;
    font-size: var(--fs-card);
    font-weight: 600;
    line-height: 1.25;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    color: var(--ink);
    overflow-wrap: anywhere;
  }

  /* The shelf's card, at the shelf's size: choosing a template is choosing off
     a shelf of covers, so it is drawn the same way. A grid rather than a
     scrolling row, because comparison is the whole task here and a carousel
     hides half of what is being compared. */
  .template-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, var(--shelf-thumb-w));
    justify-content: start;
    gap: var(--grid-gap-y) var(--grid-gap-x);
  }

  /* Room for the ring, which is drawn outside the page so that it never covers
     a millimetre of what is being chosen. Positioned because the magnifier is
     placed against it — see .template-look. */
  .template-card {
    position: relative;
    border: 0;
    border-radius: 0;
    overflow: visible;
    width: var(--shelf-thumb-w);
  }

  /* Centred, because the page above is: a template's paper is whatever shape it
     was saved at, and a name set flush to the left of the box would hang off
     the edge of a page narrower than the box every time. */
  .template-open {
    display: flex;
    flex-direction: column;
    gap: var(--space-md);
    width: 100%;
    padding: 0;
    border: 0;
    background: none;
    text-align: center;
    cursor: pointer;
  }

  .template-page {
    display: grid;
    place-items: end center;
    width: var(--shelf-thumb-w);
    height: var(--shelf-thumb-h);
  }

  /* The ring, in the three states that wear one. It is not drawn here: it is
     handed down as --cover-ring and lands on the page inside — see
     TemplateCover — so that a landscape template is ringed at its own edges and
     not at the box's. The order is the precedence, all three being equally
     specific: the pointer, then the keyboard, then the choice, which outranks
     both because it is the only one that persists.

     Nothing is written on the card to say it is chosen: there is one accent
     ring on the screen and it is unmistakable. */
  .template-card:hover .template-page {
    --cover-ring: 0 0 0 2px var(--line-3);
  }

  .template-open:focus-visible .template-page {
    --cover-ring: 0 0 0 2px var(--accent-line);
  }

  .template-card.chosen .template-page {
    --cover-ring: 0 0 0 2px var(--accent-strong);
  }

  /* The card is the page, so the focus ring is the page's; a box drawn round
     the caption as well would be a rectangle nothing on the card matches. */
  .template-open:focus-visible {
    outline: none;
  }

  /* The closer look, resting on the page's top corner — the corner of a page
     with least on it — and only while the pointer is on the card. A card is
     read by its picture, and a control parked on every one of them all the time
     is nine small buttons on a shelf that is meant to be nine pages.

     Its own square of the interface rather than a shape cut into the paper:
     what is underneath is the thing being judged, and it should look like
     something set down on the page rather than something missing from it. */
  .template-look {
    position: absolute;
    z-index: 1;
    display: grid;
    place-items: center;
    width: 1.375rem;
    height: 1.375rem;
    padding: 0;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius-sm);
    background: var(--card);
    color: var(--ink-2);
    box-shadow: var(--shadow-md);
    opacity: 0;
    cursor: zoom-in;
    transition:
      opacity var(--duration),
      color var(--duration),
      border-color var(--duration);
  }

  .template-card:hover .template-look {
    opacity: 1;
  }

  .template-look:hover {
    border-color: var(--line-3);
    color: var(--ink);
  }

  /* Nothing hovers on a touch screen, so nothing would ever reveal it. */
  @media (hover: none) {
    .template-look {
      opacity: 1;
    }
  }

  /* Never compiled when it was kept. Set in the page's own shape so the shelf
     keeps its rhythm, and quiet, because it is the absence of a picture rather
     than a warning about one. */
  .template-blank {
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    border: var(--bw) dashed var(--line-2);
    border-radius: var(--radius);
    box-shadow: var(--cover-ring, 0 0 #0000);
    color: var(--ink-3);
    font-size: var(--fs-meta);
    transition: box-shadow var(--duration);
  }

  /* The caption, centred under the paper, which is how every page-shaped card
     in Press is captioned — see .pinned-name. Centred on the cell and the page
     both, the two having the same middle: the page is centred in the cell above
     it. */
  .template-lines {
    display: grid;
    justify-items: center;
    gap: var(--space-xs);
    width: 100%;
    min-width: 0;
    text-align: center;
  }

  /* The quiet tier, in the interface's own face. The library's .where is
     monospace because it is a path and is read character by character; this is
     a count, and setting it in the same face made a caption look like a
     fragment of source. */
  .template-files {
    display: block;
    width: 100%;
    color: var(--ink-3);
    font-size: var(--fs-meta);
    line-height: 1.2;
    text-align: center;
  }

  /* Wide enough for three pages across, which is as many as can be compared
     without the eye having to travel. The shelf scrolls inside; the questions
     under it do not, so they are in the same place whatever is above them. */
  dialog.gallery {
    width: min(38rem, calc(100% - 2rem));
  }

  .gallery-shelf {
    max-height: min(26rem, 45vh);
    overflow-y: auto;
    /* Room for the selection ring, which is drawn outside the page and would
       otherwise be clipped by the scroller's edge. */
    padding: 3px;
    margin-bottom: var(--space-lg);
  }

  .creation-options {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }

  .gallery-empty {
    margin-bottom: var(--space-lg);
  }

  /* The closer look is the window: no card, no border, no shadow of its own.
     Everything a dialog is drawn with would be a frame around a page, and a
     page has an edge already — the only thing wanted here is the paper, as
     large as the window will hold it, on a room turned down. */
  dialog.look {
    width: 100vw;
    height: 100vh;
    max-width: none;
    max-height: none;
    padding: 0;
    overflow: hidden;
    border: 0;
    border-radius: 0;
    background: transparent;
    box-shadow: none;
  }

  /* Deeper than a dialog's, because what is behind this one is a dialog: a
     white page laid over a card of near-white keeps no edge, however good the
     shadow under it. */
  dialog.look::backdrop {
    background: var(--backdrop-deep);
  }

  .look-away {
    display: block;
    width: 100%;
    height: 100%;
    padding: 0;
    border: 0;
    background: none;
    cursor: zoom-out;
  }

  .look-away:focus-visible {
    outline: none;
  }

  .look-scroll {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow-y: auto;
    /* The gallery is behind this and does not scroll with it. */
    overscroll-behavior: contain;
  }

  /* `margin: auto` and not `justify-content`, which is the difference between a
     single page sitting in the middle of the window and a long template having
     its first page cut off above the top of the scroller. An auto margin gives
     up when there is no room to give; centring does not.

     The gutter between pages is the reader's own — see PdfPage — because this
     is the same column of paper. The corners are square for the same reason.

     The padding is LOOK_MARGIN, which is what the pages were sized to leave. */
  .look-column {
    --cover-radius: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-lg);
    margin: auto;
    padding-block: 3.5rem;
  }

  /* After the last page: which template this was, and how long. The gallery
     behind is covered, so the name has to be here; everything else about it was
     on the card. */
  .look-name {
    color: var(--ink);
    font-size: var(--fs-card);
    font-weight: 600;
  }

  /* The same eyebrow the settings sections wear. */
  dialog.gallery h3 {
    margin-bottom: var(--space-md);
    color: var(--ink-3);
    font-size: var(--fs-label);
    font-weight: var(--fw-label);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }

  @media (max-width: 34rem) {
    .creation-options {
      grid-template-columns: 1fr;
    }
  }

  /* The name takes the room, the folder takes what it needs, and they sit on
     one line because they are one sentence: call it this, put it there. */
  .create-row {
    display: flex;
    align-items: flex-end;
    gap: var(--space-md);
    margin-bottom: var(--space-lg);
  }

  .create-field {
    display: grid;
    gap: var(--space-xs);
    margin: 0;
    color: var(--ink-3);
    font-size: var(--fs-meta);
  }

  /* The name takes what is left; the folder takes what it needs. A folder path
     is long and a document name is short, and giving them equal room would
     leave the name in a field it could never fill. */
  .create-field:not(.folder) {
    flex: 1;
    min-width: 0;
  }

  .create-field.folder {
    min-width: 0;
    max-width: 40%;
  }

  /* The extension sits inside the field, in the field's own colours, because it
     is part of the name — just not the part anyone types. */
  .create-name {
    display: flex;
    align-items: center;
    gap: 0.15rem;
    padding-right: 0.6rem;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius-sm);
    background: var(--paper);
  }

  .create-name input {
    flex: 1;
    min-width: 0;
    margin: 0;
    border: 0;
    background: none;
  }

  .create-name input:focus {
    outline: none;
  }

  .create-extension {
    flex: none;
    color: var(--ink-3);
    font-family: var(--font-mono);
    font-size: var(--fs-meta);
  }

  .create-folder {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
    padding: 0.5rem 0.6rem;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius-sm);
    background: var(--paper);
    color: var(--ink-2);
    font-family: var(--font-sans);
    font-size: var(--fs-menu);
    line-height: 1;
    cursor: pointer;
    transition:
      border-color var(--duration),
      color var(--duration);
  }

  .create-folder:hover {
    border-color: var(--line-3);
    color: var(--ink);
  }

  /* A path is read from its end — the folder you are putting this in is the last
     component, not the first — so it is the front that is dropped. */
  .create-folder span {
    overflow: hidden;
    direction: rtl;
    text-align: left;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
