<script lang="ts">
  /* Settings: the preferences that belong to Press rather than to a document.

     Nothing here has a Save button. Theme and icon are visual and want to be
     seen at once, and a dialog where two of three controls apply themselves
     and the third waits for a button is a dialog you have to read. The command
     is written when the field is left, which is the same rule stated for a
     control that cannot show its effect.

     Opened with what it shows already fetched, so it never appears half
     filled in. */

  import type { Component } from 'svelte';
  import { Monitor, Moon, Sun } from '@lucide/svelte';
  import { api } from '$lib/api';
  import { fail, notify } from '$lib/messages.svelte';
  import { modal } from '$lib/modal';
  import { theme, type ThemePreference } from '$lib/theme.svelte';
  import { ICON_CHOICES, type EditorCommand, type IconChoice } from '$lib/types';

  let {
    command,
    icon,
    busy = false,
    onclose
  }: {
    /** The editor command as stored, with the default it stands in for. */
    command: EditorCommand;
    icon: IconChoice;
    busy?: boolean;
    onclose: () => void;
  } = $props();

  /** The three theme preferences, each with the thing it looks like. */
  const THEME_OPTIONS: Array<{ value: ThemePreference; label: string; icon: Component }> = [
    { value: 'light', label: 'Light', icon: Sun },
    { value: 'dark', label: 'Dark', icon: Moon },
    { value: 'system', label: 'System', icon: Monitor }
  ];

  /// What the Editor button runs. A command line rather than a list of editors
  /// Press knows: an editor Press has never heard of is one line here, and
  /// anything that will not fit on one line fits in a script, which is a better
  /// home for it than a table inside Press.
  ///
  /// An unset command shows the default it is standing in for, so that editing
  /// it is a change to something visible rather than to a blank.
  // svelte-ignore state_referenced_locally
  let editorCommand = $state(command.command);
  /// What is stored, as far as this dialog knows. Moves on when the field is
  /// written, so a second blur has nothing left to do.
  // svelte-ignore state_referenced_locally
  let editorDefault = $state<EditorCommand>(command);
  // svelte-ignore state_referenced_locally
  let iconChoice = $state<IconChoice>(icon);

  /// The command, written when the field is left rather than on every keystroke.
  ///
  /// Called from blur and from closing the dialog, which between them cover
  /// every way of being finished with it — and it is worth nothing to write the
  /// same string twice, so neither has to know about the other.
  async function saveEditorCommand() {
    const wanted = editorCommand.trim();
    if (wanted === editorDefault.command.trim()) return;
    try {
      await api.setEditorCommand(wanted);
      // What was saved is what the field now stands for, so a second blur has
      // nothing left to do.
      editorDefault = { ...editorDefault, command: wanted };
    } catch (reason) {
      fail(reason);
    }
  }

  /// The icon, applied at once — it is a picture, and a picture you have to
  /// press Save to see is a picture you cannot choose between.
  async function chooseIcon(name: IconChoice) {
    const previous = iconChoice;
    iconChoice = name;
    try {
      const shown = await api.setIconChoice(name);
      if (!shown) {
        // A development build is a bare binary with no bundle to write on. The
        // choice is stored and will be worn by the next installed Press.
        notify('Icon saved. A development build has no bundle to show it on.');
      }
    } catch (reason) {
      iconChoice = previous;
      fail(reason);
    }
  }

  async function close() {
    await saveEditorCommand();
    onclose();
  }
</script>

<dialog class="settings" use:modal={close} aria-labelledby="settings-title">
  <h2 id="settings-title">Settings</h2>

  <section>
    <h3>Theme</h3>
    <!-- The same switch as ⌃r, not a second preference that has to agree with
         it. System is not a third way for a page to look — it is declining to
         choose, and it follows macOS for as long as it is set. -->
    <div class="choices" role="radiogroup" aria-label="Theme">
      {#each THEME_OPTIONS as option (option.value)}
        <button
          class="choice"
          role="radio"
          aria-checked={theme.preference === option.value}
          onclick={() => (theme.preference = option.value)}
        >
          <option.icon size={16} strokeWidth={1.75} aria-hidden="true" />
          {option.label}
        </button>
      {/each}
    </div>
  </section>

  <section>
    <h3>Icon</h3>
    <!-- No names under these. They are pictures, and the thing a picture is
         called adds nothing to choosing between three of them you can see. -->
    <div class="choices" role="radiogroup" aria-label="Dock icon">
      {#each ICON_CHOICES as name (name)}
        <button
          class="choice tile"
          role="radio"
          aria-checked={iconChoice === name}
          onclick={() => chooseIcon(name)}
          title={name}
          aria-label={name}
        >
          <img src="/icons/press-{name}.png" alt="" width="72" height="72" />
          <span class="dot" aria-hidden="true"></span>
        </button>
      {/each}
    </div>
  </section>

  <section>
    <h3>Editor</h3>
    <label>
      Command
      <input
        bind:value={editorCommand}
        spellcheck="false"
        autocapitalize="off"
        autocorrect="off"
        onblur={saveEditorCommand}
        onkeydown={(event) => {
          if (event.key === 'Enter') saveEditorCommand();
        }}
      />
    </label>
    <p class="quiet">
      What the Editor button runs. <code>{'{file}'}</code> is the document and
      <code>{'{dir}'}</code> its folder; quote anything with a space in it. Left empty, the
      document opens in whatever your system uses for that kind of file.
    </p>
    <p class="quiet">
      Clicking a build error runs it too, with <code>{'{file}'}</code> as the file the error is
      in and <code>{'{line}'}</code> as its line, as in <code>nvim +{'{line}'} {'{file}'}</code>
      or <code>code --goto {'{file}'}:{'{line}'}</code>.
    </p>
    {#if editorCommand.trim() !== editorDefault.suggested}
      <p class="quiet">
        Suggested for this machine:
        <button class="link" onclick={() => (editorCommand = editorDefault.suggested)}>
          <code>{editorDefault.suggested}</code>
        </button>
      </p>
    {/if}
  </section>

  <!-- One button, and it is not Save. Everything here has applied itself
       already; this only puts the dialog away. -->
  <div class="dialog-actions">
    <button onclick={close} disabled={busy}>Done</button>
  </div>
</dialog>

<style>
  .quiet {
    color: var(--ink-3);
  }

  /* The library's borderless text button, for the one suggestion in here. */
  .link {
    padding: 0.15rem 0.35rem;
    border: 0;
    border-radius: var(--radius-xs);
    background: none;
    color: var(--ink-2);
    cursor: pointer;
  }

  .link:hover:not(:disabled) {
    background: var(--paper-2);
    color: var(--ink);
  }

  /* Settings is the one dialog with more than one subject in it, so the
     subjects are ruled off from each other rather than left to run together.
     The last one is not, because a line above the Done button would read as a
     fourth section that had lost its heading. */
  dialog.settings section {
    padding-bottom: 1rem;
    margin-bottom: 1rem;
    border-bottom: var(--bw) solid var(--line-1);
  }

  dialog.settings section:last-of-type {
    padding-bottom: 0;
    border-bottom: 0;
  }

  /* The same eyebrow a field label wears — these name a group of controls
     rather than announcing a new part of the interface. */
  dialog.settings h3 {
    margin-bottom: 0.5rem;
    color: var(--ink-3);
    font-size: var(--fs-label);
    font-weight: var(--fw-label);
    letter-spacing: var(--tracking-label);
    text-transform: uppercase;
  }

  dialog.settings section p:last-child {
    margin-bottom: 0;
  }

  /* Two levels, told apart. Uppercase is what names a section; a field inside
     one wearing the same treatment made EDITOR and COMMAND look like siblings
     when the first contains the second. Scoped to Settings, because a dialog
     that is one question and one field has no hierarchy to show. */
  dialog.settings label {
    color: var(--ink-3);
    font-size: var(--fs-meta);
    font-weight: 500;
    letter-spacing: normal;
    text-transform: none;
  }

  /* Left, on the margin the headings and fields already keep. These were
     centred when the dialog was wide enough for two columns; at one column,
     centring them would set three buttons adrift above a field that starts at
     the edge. */
  .choices {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-bottom: 0.75rem;
  }

  /* Radios drawn as what they select. A native radio beside a word would put
     the choice in the dot rather than in the thing chosen, which is the wrong
     way round when the thing chosen is a picture. */
  .choice {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.45rem 0.7rem;
    border: var(--bw) solid var(--line-2);
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--ink-2);
    font-family: var(--font-sans);
    font-size: var(--fs-menu);
    line-height: 1;
    cursor: pointer;
    transition:
      border-color var(--duration),
      color var(--duration);
  }

  .choice:hover {
    color: var(--ink);
    border-color: var(--line-3);
  }

  .choice[aria-checked='true'] {
    border-color: var(--accent);
    color: var(--ink);
  }

  /* The word options carry their selection in the ring, having no room below
     for anything else. The tiles below carry theirs in the dot instead. */
  .choice:not(.tile)[aria-checked='true'] {
    box-shadow: inset 0 0 0 var(--bw) var(--accent);
  }

  /* A tile is the picture and nothing else — no frame competing with artwork
     that already has its own edge, and no name under it. What marks the chosen
     one is the dot, which is the only part of a radio worth keeping when the
     label has gone. */
  .choice.tile {
    flex-direction: column;
    gap: 0.55rem;
    padding: 0;
    border: 0;
  }

  .choice.tile img {
    display: block;
    width: 72px;
    height: 72px;
    /* Unchosen tiles step back rather than grey out: these are the same picture
       in three colourways, and a desaturated preview of a colour is no preview
       at all. */
    opacity: 0.55;
    transition: opacity var(--duration);
  }

  .choice.tile:hover img,
  .choice.tile[aria-checked='true'] img {
    opacity: 1;
  }

  /* One radio mark, wherever a radio is drawn. */
  .dot {
    flex: none;
    width: 0.75rem;
    height: 0.75rem;
    border: var(--bw) solid var(--line-3);
    border-radius: 50%;
    background: transparent;
    transition:
      border-color var(--duration),
      background var(--duration),
      box-shadow var(--duration);
  }

  .choice.tile:hover .dot {
    border-color: var(--ink-3);
  }

  /* Filled in the accent, with the ring holding a hairline of the card between
     dot and border — the way a real radio is drawn. */
  .choice.tile[aria-checked='true'] .dot {
    border-color: var(--accent);
    background: var(--accent);
    box-shadow: inset 0 0 0 2px var(--card);
  }
</style>
