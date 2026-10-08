/**
 * restask Obsidian plugin (docs/spec/integrations.md §15): commands, suggestions,
 * settings, hiding the `🆔` token, registering new tasks where they are typed, and
 * starting a new line of a TODO section with a checkbox.
 *
 * This file isolates every Obsidian API call; markdown.ts, modal.ts, toggle.ts and
 * conceal.ts stay API-free and unit-testable, and editor.ts uses CodeMirror only. The plugin only edits Markdown — on the desktop and on
 * the phone alike. The restask daemon picks the edits up from the vault (carried by
 * file sync) and talks to the CalDAV server; the plugin never does.
 */

import {
	App,
	Editor,
	EditorPosition,
	EditorSuggest,
	EditorSuggestContext,
	EditorSuggestTriggerInfo,
	MarkdownView,
	Notice,
	Platform,
	Plugin,
	SuggestModal,
	TFile,
	editorInfoField,
	normalizePath,
	type MarkdownFileInfo,
} from "obsidian";
import type { EditorState, Extension } from "@codemirror/state";
import { stripUid } from "./conceal";
import { taskFiling, taskStart, uidConcealment, type Carried, type DroppedMirror, type FilingHost } from "./editor";
import {
	completedByUid,
	declaresRoot,
	disclaimed,
	inView,
	isSealed,
	mirrorDropped,
	mirrorEdited,
	mirrorLine,
	mirrorSource,
	mirrored,
	putBack,
	refiled,
	removed,
	resealed,
	routesItself,
	settled,
	taskIndex,
	toggledLines,
	uidGenerator,
	viewOf,
	type NoteKind,
} from "./filing";
import { checkOffset, parseLine } from "./markdown";
import { suggestionsFor, triggerAt, type Suggestion } from "./modal";
import { DEFAULT_SETTINGS, RestaskSettingTab, type RestaskSettings } from "./settings";
import { toggleDone } from "./toggle";

/** Device-local calendar date `YYYY-MM-DD` (§3.3) — the wall clock is read only here. */
function localToday(now: Date): string {
	const m = String(now.getMonth() + 1).padStart(2, "0");
	const d = String(now.getDate()).padStart(2, "0");
	return `${now.getFullYear()}-${m}-${d}`;
}

function renderSuggestion(suggestion: Suggestion, el: HTMLElement): void {
	el.createEl("div", { text: suggestion.keyword });
	el.createEl("small", { text: suggestion.insert.trim() });
}

/** Suggestions while typing (§15.2): opens on a keyword fragment in a task line. */
class MetadataSuggest extends EditorSuggest<Suggestion> {
	private readonly plugin: RestaskPlugin;

	constructor(app: App, plugin: RestaskPlugin) {
		super(app);
		this.plugin = plugin;
	}

	onTrigger(cursor: EditorPosition, editor: Editor): EditorSuggestTriggerInfo | null {
		if (!this.plugin.settings.suggestWhileTyping) return null;
		const before = editor.getLine(cursor.line).slice(0, cursor.ch);
		const trigger = triggerAt(before, localToday(new Date()));
		if (trigger === undefined) return null;
		return { start: { line: cursor.line, ch: trigger.start }, end: cursor, query: trigger.query };
	}

	getSuggestions(context: EditorSuggestContext): Suggestion[] {
		return suggestionsFor(context.query, localToday(new Date()));
	}

	renderSuggestion(suggestion: Suggestion, el: HTMLElement): void {
		renderSuggestion(suggestion, el);
	}

	selectSuggestion(suggestion: Suggestion): void {
		const context = this.context;
		if (context === null) return;
		// The typed fragment is replaced by the metadata it stood for.
		context.editor.replaceRange(suggestion.insert, context.start, context.end);
		const ch = context.start.ch + suggestion.insert.length;
		context.editor.setCursor({ line: context.start.line, ch });
		this.close();
	}
}

/** The same suggestions on demand (§15.2), for the command palette and hotkeys. */
class MetadataModal extends SuggestModal<Suggestion> {
	private readonly editor: Editor;
	private readonly today: string;

	constructor(app: App, editor: Editor, today: string) {
		super(app);
		this.editor = editor;
		this.today = today;
		this.setPlaceholder("high, medium, low, due, start, scheduled, repeat, created, today, tomorrow…");
	}

	getSuggestions(query: string): Suggestion[] {
		return suggestionsFor(query, this.today);
	}

	renderSuggestion(suggestion: Suggestion, el: HTMLElement): void {
		renderSuggestion(suggestion, el);
	}

	onChooseSuggestion(suggestion: Suggestion): void {
		const cursor = this.editor.getCursor();
		const before = this.editor.getLine(cursor.line).slice(0, cursor.ch);
		const needsSpace = before.length > 0 && !/[ \t]$/.test(before);
		this.editor.replaceSelection(`${needsSpace ? " " : ""}${suggestion.insert}`);
	}
}

/**
 * Reading view (§15.5): drops the `🆔` token from the text of each rendered task item.
 * Only the rendering changes; the note is not touched.
 */
function hideUids(el: HTMLElement): void {
	for (const item of Array.from(el.querySelectorAll("li.task-list-item"))) {
		const walker = document.createTreeWalker(item, NodeFilter.SHOW_TEXT);
		for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
			// The text of a nested item belongs to that item.
			if (node.parentElement?.closest("li") !== item) continue;
			const text = node.nodeValue ?? "";
			const stripped = stripUid(text);
			if (stripped === text) continue;
			node.nodeValue = stripped;
			break;
		}
	}
}

/** The restask plugin (§15.4). */
export default class RestaskPlugin extends Plugin {
	settings: RestaskSettings = DEFAULT_SETTINGS;
	/** Registered once; its contents follow the `hideTaskIds` setting. */
	private readonly editorExtensions: Extension[] = [];
	/** Fresh task UIDs (§3.1); one generator, so UIDs made together keep their order. */
	private readonly newUid = uidGenerator(
		() => Date.now(),
		(count) => crypto.getRandomValues(new Uint8Array(count)),
	);
	/**
	 * Vault path of the TODO.md view (`inbox_file` in `restask.toml`, §14.1); undefined in
	 * a vault that is not set up. The view is known by this path and by nothing in it,
	 * as in the engine: a note that happens to hold a line of a view is a note.
	 */
	private inboxPath: string | undefined;
	/** The plugin's edits of files that are not in the editor at hand run one after the other. */
	private fileJobs: Promise<void> = Promise.resolve();
	/** Task lines taken out of their notes because their mirror line was deleted (§7.1), by UID: where each one was. */
	private readonly dropped = new Map<string, { path: string; index: number; line: string }>();

	async onload(): Promise<void> {
		await this.loadSettings();
		await this.loadInboxPath();
		// `restask.toml` may arrive, or change, through the file sync while the app is open.
		this.registerEvent(this.app.workspace.on("file-open", () => void this.loadInboxPath()));
		this.addSettingTab(new RestaskSettingTab(this.app, this));
		this.registerEditorSuggest(new MetadataSuggest(this.app, this));
		this.registerEditorExtension(this.editorExtensions);
		const host = this.filingHost();
		this.registerEditorExtension(taskFiling(host));
		this.registerEditorExtension(taskStart(host));
		// Reading view changes a checkbox without the editor: seen here, before Obsidian handles the tap.
		this.registerDomEvent(document, "click", (event) => this.onPreviewClick(event), { capture: true });
		this.registerMarkdownPostProcessor((el) => {
			if (this.settings.hideTaskIds) hideUids(el);
		});
		this.applyConcealment();

		this.addCommand({
			id: "toggle-task-done",
			name: "Toggle task done",
			editorCallback: (editor, ctx) => this.toggleTaskDone(editor, ctx),
		});

		this.addCommand({
			id: "add-metadata",
			name: "Add metadata",
			editorCallback: (editor) => {
				new MetadataModal(this.app, editor, localToday(new Date())).open();
			},
		});
	}

	async saveSettings(): Promise<void> {
		await this.saveData(this.settings);
	}

	/** Brings open editors and reading views in line with the `hideTaskIds` setting (§15.5). */
	applyConcealment(): void {
		this.editorExtensions.length = 0;
		if (this.settings.hideTaskIds) this.editorExtensions.push(uidConcealment());
		this.app.workspace.updateOptions();
		this.app.workspace.iterateAllLeaves((leaf) => {
			if (leaf.view instanceof MarkdownView) leaf.view.previewMode.rerender(true);
		});
	}

	/** Whether this device settles tasks itself (§15.6, the `settleTasks` setting). */
	private settlesHere(): boolean {
		const mode = this.settings.settleTasks;
		return mode === "always" || (mode === "mobile" && Platform.isMobile);
	}

	/** The plugin's side of §15.6 for the editor: the setting, the vault, the clock and the UIDs. */
	private filingHost(): FilingHost {
		const fileOf = (state: EditorState): TFile | undefined => {
			const file = state.field(editorInfoField, false)?.file;
			return file instanceof TFile ? file : undefined;
		};
		return {
			enabled: () => this.settlesHere(),
			startsTasks: () => this.settings.startTasks,
			isInboxView: (state) => {
				const file = fileOf(state);
				return file !== undefined && file.path === this.inboxPath;
			},
			inheritsList: (state) => {
				const file = fileOf(state);
				return file !== undefined && this.inheritsList(file);
			},
			doneHeading: () => this.settings.doneHeading,
			today: () => localToday(new Date()),
			uid: this.newUid,
			carryMirror: (state, uid, was, now) => {
				const view = fileOf(state);
				if (view === undefined) return Promise.resolve("failed");
				return this.queued(() => this.carryMirror(view, uid, was, now), "failed");
			},
			dropMirror: (state, uid, line) => {
				const view = fileOf(state);
				if (view === undefined) return Promise.resolve("failed");
				return this.queued(() => this.dropMirror(view, uid, line), "failed");
			},
			mirrorReturned: (_state, uid) => {
				void this.queued(() => this.restoreDropped(uid), undefined);
			},
			noteTaskSettled: (state, uid) => {
				const note = fileOf(state);
				if (note === undefined) return;
				const lines = state.doc.toJSON();
				void this.queued(() => this.syncMirrors(note, lines, uid), undefined);
			},
		};
	}

	/** Runs `job` after the file jobs queued before it; a job that throws yields `fallback`. */
	private queued<T>(job: () => Promise<T>, fallback: T): Promise<T> {
		const result = this.fileJobs.then(job).catch((error: unknown) => {
			console.error("restask:", error);
			return fallback;
		});
		this.fileJobs = result.then(() => undefined);
		return result;
	}

	/**
	 * `true` when a note in the file's folder, or in one above it, declares
	 * `restask-list-root` (§5.2) — read from Obsidian's frontmatter cache.
	 */
	private inheritsList(file: TFile): boolean {
		for (let folder = file.parent; folder !== null; folder = folder.parent) {
			for (const child of folder.children) {
				if (!(child instanceof TFile) || child.extension !== "md") continue;
				const root: unknown = this.app.metadataCache.getFileCache(child)?.frontmatter?.["restask-list-root"];
				if ((typeof root === "string" && root.trim() !== "") || typeof root === "number") return true;
			}
		}
		return false;
	}

	/** How a note takes part (§15.6), or undefined when it is local-only (invariant 4). */
	private noteKind(file: TFile, lines: readonly string[]): NoteKind | undefined {
		const inboxView = file.path === this.inboxPath;
		if (!inboxView && !routesItself(lines) && !this.inheritsList(file)) return undefined;
		return { inboxView, doneHeading: this.settings.doneHeading, root: !inboxView && declaresRoot(lines) };
	}

	/**
	 * The files whose view shows the tasks of `note` (§7, §7.6): the root notes of its
	 * folder and of the folders above it — read from Obsidian's frontmatter cache — and
	 * TODO.md. Never `note` itself, nor the file `except`.
	 */
	private async viewsOf(note: TFile, except?: string): Promise<TFile[]> {
		const views: TFile[] = [];
		const inbox = await this.inboxFile();
		for (let folder = note.parent; folder !== null && note.path !== inbox?.path; folder = folder.parent) {
			for (const child of folder.children) {
				if (!(child instanceof TFile) || child.extension !== "md") continue;
				const root: unknown = this.app.metadataCache.getFileCache(child)?.frontmatter?.["restask-list-root"];
				if ((typeof root === "string" && root.trim() !== "") || typeof root === "number") views.push(child);
			}
		}
		if (inbox !== undefined) views.push(inbox);
		return views.filter((view) => view.path !== note.path && view.path !== except);
	}

	/** Replaces the content of `file` by `next` if it still is `expected`; `false` when something else got there first. */
	private async rewrite(file: TFile, expected: string, next: string): Promise<boolean> {
		let written = false;
		await this.app.vault.process(file, (disk) => {
			if (disk !== expected) return disk;
			written = true;
			return next;
		});
		return written;
	}

	/** Reads where the vault keeps its TODO.md view: `inbox_file` of `restask.toml`, `TODO.md` when the config names none. */
	private async loadInboxPath(): Promise<void> {
		try {
			const config = await this.app.vault.adapter.read("restask.toml");
			this.inboxPath = normalizePath(/^[ \t]*inbox_file[ \t]*=[ \t]*"([^"\n]*)"/m.exec(config)?.[1] ?? "TODO.md");
		} catch {
			// No vault config: restask is not set up here, and no file is its view.
			this.inboxPath = undefined;
		}
	}

	/** The vault's inbox file (§14.1), when the vault is set up and the file is there. */
	private async inboxFile(): Promise<TFile | undefined> {
		await this.loadInboxPath();
		const file = this.inboxPath === undefined ? null : this.app.vault.getAbstractFileByPath(this.inboxPath);
		return file instanceof TFile ? file : undefined;
	}

	/**
	 * The routed note that has the task `uid`, with its text: the note a mirror line
	 * links to (`stem`), else whichever note of the vault has it. Notes with CRLF line
	 * endings are not offered: the daemon keeps their endings, the plugin would not.
	 */
	private async sourceNote(view: TFile, stem: string | undefined, uid: string): Promise<{ note: TFile; text: string; lines: string[] } | undefined> {
		const tried = new Set<string>([view.path]);
		const attempt = async (note: TFile): Promise<{ note: TFile; text: string; lines: string[] } | undefined> => {
			tried.add(note.path);
			const text = await this.app.vault.read(note);
			if (!text.includes(uid) || text.includes("\r")) return undefined;
			const lines = text.split("\n");
			if (this.noteKind(note, lines)?.inboxView !== false) return undefined;
			// A mirror line in the view of a root note is not the task (§7.6).
			return taskIndex(lines, uid, this.settings.doneHeading) >= 0 ? { note, text, lines } : undefined;
		};
		const linked = stem === undefined ? null : this.app.metadataCache.getFirstLinkpathDest(stem, view.path);
		if (linked instanceof TFile && !tried.has(linked.path)) {
			const found = await attempt(linked);
			if (found !== undefined) return found;
		}
		for (const note of this.app.vault.getMarkdownFiles()) {
			if (tried.has(note.path)) continue;
			const found = await attempt(note);
			if (found !== undefined) return found;
		}
		return undefined;
	}

	/**
	 * Carries what the user changed on a mirror line of the view to the task's source
	 * note (§7.1): the fields that differ between `was` and `now`, and a checked box.
	 * `"unclaimed"` when no routed note has the task.
	 */
	private async carryMirror(view: TFile, uid: string, was: string | undefined, now: string): Promise<Carried> {
		const checked = parseLine(now)?.draft.checked === true;
		// Without the line as it was, only a completion is unambiguous.
		if (was === undefined && !checked) return "failed";
		const found = await this.sourceNote(view, mirrorSource(now) ?? (was === undefined ? undefined : mirrorSource(was)), uid);
		if (found === undefined) return "unclaimed";
		const heading = this.settings.doneHeading;
		let lines = found.lines;
		if (was !== undefined) {
			const edited = mirrorEdited(lines, uid, was, now, found.note.basename, heading);
			if (edited === "absent") return "unclaimed";
			lines = edited ?? lines;
		}
		if (checked) {
			const completed = completedByUid(lines, uid, localToday(new Date()), heading);
			if (completed === "absent") return "unclaimed";
			lines = completed ?? lines;
		}
		if (lines === found.lines) return "carried";
		lines = this.ownLineSettled(found.note, found.lines, lines, uid);
		if (!(await this.rewrite(found.note, found.text, lines.join("\n")))) return "failed";
		await this.syncMirrors(found.note, lines, uid, view.path);
		return "carried";
	}

	/**
	 * A source note the plugin has edited the task `uid` in, `before` and `after` the
	 * edit: when the task is a line in the view of a root note (§7.6) it is filed in the
	 * section it now belongs to, and a view that was sealed is sealed again — the edit is
	 * the plugin's own, carried from another view.
	 */
	private ownLineSettled(note: TFile, before: readonly string[], after: string[], uid: string): string[] {
		const kind = this.noteKind(note, before);
		const sealed = kind !== undefined && isSealed(before, viewOf(before, kind));
		if (kind?.root !== true || viewOf(before, kind) === undefined) return after;
		let lines = after;
		const at = taskIndex(lines, uid, this.settings.doneHeading);
		const view = viewOf(lines, kind);
		if (at >= 0 && inView(view, at)) lines = refiled(lines, at, true, view) ?? lines;
		return sealed ? resealed(lines, viewOf(lines, kind)) : lines;
	}

	/** Deletes the task `uid` from its source note: its mirror line `line` was deleted from the view (§7.1). */
	private async dropMirror(view: TFile, uid: string, line: string): Promise<DroppedMirror> {
		const found = await this.sourceNote(view, mirrorSource(line), uid);
		const dropped = found === undefined ? undefined : mirrorDropped(found.lines, uid, line, found.note.basename, this.settings.doneHeading);
		if (found === undefined || dropped === undefined) return "unclaimed";
		if ("restore" in dropped) return dropped;
		// A line of a view's own takes a section it leaves empty with it, as the render would (§7.6).
		const kind = this.noteKind(found.note, found.lines);
		const shown = kind === undefined ? undefined : viewOf(found.lines, kind);
		const without = inView(shown, dropped.index) ? removed(found.lines, dropped.index, shown) : dropped.lines;
		const lines = this.ownLineSettled(found.note, found.lines, without, uid);
		if (!(await this.rewrite(found.note, found.text, lines.join("\n")))) return "failed";
		this.dropped.set(uid, { path: found.note.path, index: dropped.index, line: dropped.line });
		await this.syncMirrors(found.note, lines, uid, view.path);
		return "deleted";
	}

	/** Puts the task `uid` back into the note {@link dropMirror} took it out of: its mirror line is in the view again. */
	private async restoreDropped(uid: string): Promise<void> {
		const was = this.dropped.get(uid);
		this.dropped.delete(uid);
		const note = was === undefined ? null : this.app.vault.getAbstractFileByPath(was.path);
		if (was === undefined || !(note instanceof TFile)) return;
		const text = await this.app.vault.read(note);
		if (text.includes("\r")) return;
		const before = text.split("\n");
		const put = putBack(before, uid, was.index, was.line);
		if (put === undefined) return;
		const after = this.ownLineSettled(note, before, put, uid);
		if (await this.rewrite(note, text, after.join("\n"))) await this.syncMirrors(note, after, uid);
	}

	/**
	 * Brings the views that show the tasks of `note` — TODO.md (§7) and the root notes
	 * above it (§7.6) — in line with its task `uid`; `lines` is the note's current text.
	 * `except` is a view that has the change already: the one it was made in.
	 */
	private async syncMirrors(note: TFile, lines: readonly string[], uid: string, except?: string): Promise<void> {
		for (const file of await this.viewsOf(note, except)) await this.syncMirror(file, note, lines, uid);
	}

	/**
	 * Brings the view in `file` in line with the task `uid` of `note`, whose current
	 * text is `lines`: its mirror line is added when the task is active and has a
	 * priority, removed when it is not, and — in a sealed view, which stays sealed
	 * (§7.2) — rewritten when the task changed.
	 */
	private async syncMirror(file: TFile, note: TFile, lines: readonly string[], uid: string): Promise<void> {
		const text = await this.app.vault.read(file);
		if (text.includes("\r")) return;
		const shown = text.split("\n");
		const kind = this.noteKind(file, shown);
		const view = kind === undefined ? undefined : viewOf(shown, kind);
		if (kind === undefined || view === undefined) return;
		const sealed = isSealed(shown, view);
		const line = mirrorLine(lines, uid, note.basename, this.settings.doneHeading);
		const after = mirrored(shown, uid, line, sealed, view);
		if (after === undefined) return;
		// A line taken out of a view that stays unsealed must not read as the user's deletion (§7.1).
		const next = sealed ? resealed(after, viewOf(after, kind)) : line === undefined ? disclaimed(after) : after;
		await this.rewrite(file, text, next.join("\n"));
	}

	/** A tap on a checkbox in reading view (§15.6): what the note looks like once Obsidian has handled it is settled. */
	private onPreviewClick(event: MouseEvent): void {
		if (!this.settlesHere()) return;
		const target = event.target;
		if (!(target instanceof HTMLElement) || !target.matches("input.task-list-item-checkbox")) return;
		for (const leaf of this.app.workspace.getLeavesOfType("markdown")) {
			const view = leaf.view;
			if (!(view instanceof MarkdownView) || view.getMode() !== "preview" || !view.containerEl.contains(target)) continue;
			const before = view.getViewData();
			window.setTimeout(() => void this.queued(() => this.settlePreview(view, before), undefined), 0);
			return;
		}
	}

	private async settlePreview(view: MarkdownView, before: string): Promise<void> {
		const file = view.file;
		const text = view.getViewData();
		if (file === null || text === before || text.includes("\r")) return;
		const lines = text.split("\n");
		const toggled = toggledLines(before.split("\n"), lines);
		const note = this.noteKind(file, lines);
		if (toggled.length !== 1 || note === undefined) return;
		const idx = toggled[0];
		const today = localToday(new Date());
		const result = settled(lines, idx, note, today, this.newUid);
		if (result === undefined) return;
		const beforeLines = before.split("\n");
		const sealedBefore = isSealed(beforeLines, viewOf(beforeLines, note));
		const shown = viewOf(lines, note);
		// Obsidian saves the tap itself; the settled note replaces exactly that version.
		await view.save();
		let after = result.lines;
		let tookOut = false;
		if (result.carry !== undefined) {
			// A checked mirror line: the completion is made in the note, the line leaves the view.
			const outcome = await this.carryMirror(file, result.carry, undefined, lines[idx]);
			if (outcome === "failed") return;
			tookOut = outcome === "carried";
			after = outcome === "carried" ? removed(lines, idx, shown) : settled(lines, idx, note, today, this.newUid, true)?.lines;
		}
		// The tap broke the seal of a sealed view; with the tap settled it is a render again (§7.2).
		if (shown !== undefined && sealedBefore) after = resealed(after ?? lines, viewOf(after ?? lines, note));
		// A line taken out of a view that stays unsealed must not read as the user's deletion (§7.1).
		else if (tookOut) after = disclaimed(after ?? lines);
		if (after !== undefined && !(await this.rewrite(file, text, after.join("\n")))) return;
		// A task of the note — a line of its own in the view of a root note too — shows in the views above it.
		if (!note.inboxView && result.mirror !== true && result.uid !== undefined) await this.syncMirrors(file, after ?? lines, result.uid);
	}

	private async loadSettings(): Promise<void> {
		this.settings = Object.assign({}, DEFAULT_SETTINGS, await this.loadData());
	}

	/**
	 * `Toggle task done` (§15.3): completes or reopens the task on the cursor line, in a
	 * note or in TODO.md alike (the daemon carries a TODO.md toggle to the source note).
	 */
	private toggleTaskDone(editor: Editor, ctx: MarkdownView | MarkdownFileInfo): void {
		const cursor = editor.getCursor().line;
		const check = checkOffset(editor.getLine(cursor));
		// Where the plugin settles tasks itself (§15.6), flipping the box is all there is to do.
		if (check !== undefined && this.settlesHere() && ctx.file !== null && this.noteKind(ctx.file, editor.getValue().split("\n")) !== undefined) {
			const checked = editor.getLine(cursor)[check] !== " ";
			editor.replaceRange(checked ? " " : "x", { line: cursor, ch: check }, { line: cursor, ch: check + 1 });
			return;
		}
		const result = toggleDone(
			editor.getValue(),
			editor.getCursor().line,
			localToday(new Date()),
			this.settings.doneHeading,
		);
		if (result === undefined) {
			new Notice("restask: the cursor is not on a task line.");
			return;
		}
		editor.setValue(result.doc);
		editor.setCursor({ line: result.line, ch: 0 });
	}
}
