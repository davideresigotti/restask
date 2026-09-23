/**
 * Taskres Obsidian plugin (docs/spec/integrations.md §15.4): commands + settings.
 *
 * This file isolates every Obsidian API call; markdown.ts, modal.ts and vtodo.ts stay
 * API-free and unit-testable (AGENTS.md §3). The plugin mutates Markdown and mirrors
 * mutations into `.restask/tasks/<uid>.ics`; it never talks CalDAV — that is the
 * daemon's job (§15).
 */

import { App, Editor, MarkdownView, Notice, Plugin, SuggestModal, TFile, normalizePath } from "obsidian";
import { linkParents, parse, parseLine, type TaskDraft } from "./markdown";
import { suggestionsFor, type Suggestion } from "./modal";
import { formatUtc, writeCacheFile, type AdapterPort, type CacheTask } from "./vtodo";
import { DEFAULT_SETTINGS, TaskresSettingTab, type TaskresSettings } from "./settings";

/** Device-local calendar date `YYYY-MM-DD` (§3.3) — wall clock is allowed only here. */
function localToday(now: Date): string {
	const m = String(now.getMonth() + 1).padStart(2, "0");
	const d = String(now.getDate()).padStart(2, "0");
	return `${now.getFullYear()}-${m}-${d}`;
}

/** Flips the checkbox of a §6.1 task line, preserving everything else byte-for-byte. */
function flipCheck(raw: string, checked: boolean): string {
	return raw.replace(/^([ \t]*[-*+][ \t]+\[)([ xX])(\])/, (_m, pre: string, _c: string, post: string) =>
		`${pre}${checked ? "x" : " "}${post}`,
	);
}

/** Appends `✅ <today>` in canonical tail position: before ➕/🆔 when present (§6.1). */
function withCompletedToken(raw: string, today: string): string {
	const insertion = `✅ ${today}`;
	const plus = raw.search(/➕[ \t]+\d{4}-\d{2}-\d{2}/);
	const anchor = plus >= 0 ? plus : raw.search(/🆔[ \t]+taskres-/);
	if (anchor >= 0) {
		return `${raw.slice(0, anchor).replace(/[ \t]+$/, "")} ${insertion} ${raw.slice(anchor)}`;
	}
	return `${raw.replace(/[ \t]+$/, "")} ${insertion}`;
}

/** Removes the `✅ <date>` token (uncomplete); the completed date is discarded per §15.4. */
function withoutCompletedToken(raw: string): string {
	return raw.replace(/[ \t]*✅[ \t]+\d{4}-\d{2}-\d{2}/, "").replace(/[ \t]+$/, "");
}

/** Autocomplete modal (§15.2): suggestions appear at ≥ 2 letters, insert on choose. */
class MetadataModal extends SuggestModal<Suggestion> {
	private readonly editor: Editor;
	private readonly today: string;

	constructor(app: App, editor: Editor, today: string) {
		super(app);
		this.editor = editor;
		this.today = today;
	}

	getSuggestions(query: string): Suggestion[] {
		return suggestionsFor(query, this.today);
	}

	renderSuggestion(suggestion: Suggestion, el: HTMLElement): void {
		el.createEl("div", { text: suggestion.keyword });
		el.createEl("small", { text: suggestion.insert.trim() });
	}

	onChooseSuggestion(suggestion: Suggestion): void {
		const cursor = this.editor.getCursor();
		const before = this.editor.getLine(cursor.line).slice(0, cursor.ch);
		const needsSpace = before.length > 0 && !/[ \t]$/.test(before);
		this.editor.replaceSelection(`${needsSpace ? " " : ""}${suggestion.insert}`);
	}
}

/** The Taskres plugin: three commands (§15.4) over the pure markdown/modal/vtodo cores. */
export default class TaskresPlugin extends Plugin {
	settings: TaskresSettings = DEFAULT_SETTINGS;

	async onload(): Promise<void> {
		await this.loadSettings();
		this.addSettingTab(new TaskresSettingTab(this.app, this));

		this.addCommand({
			id: "toggle-task-done",
			name: "Toggle task done",
			editorCallback: (editor, ctx) => {
				const file = ctx instanceof TFile ? ctx : ctx.file;
				void this.toggleTaskDone(editor, file);
			},
		});

		this.addCommand({
			id: "add-metadata",
			name: "Add metadata",
			editorCallback: (editor) => {
				new MetadataModal(this.app, editor, localToday(new Date())).open();
			},
		});

		this.addCommand({
			id: "sync-now",
			name: "Sync now",
			callback: () => {
				void this.syncNow();
			},
		});
	}

	async saveSettings(): Promise<void> {
		await this.saveData(this.settings);
	}

	private async loadSettings(): Promise<void> {
		this.settings = Object.assign({}, DEFAULT_SETTINGS, await this.loadData());
	}

	/** The Obsidian vault adapter behind the API-free AdapterPort (§15.3), paths normalized. */
	private get adapter(): AdapterPort {
		const adapter = this.app.vault.adapter;
		return {
			exists: (path) => adapter.exists(normalizePath(path)),
			mkdir: (path) => adapter.mkdir(normalizePath(path)),
			write: (path, data) => adapter.write(normalizePath(path), data),
		};
	}

	/** Best-effort cache mirror: enabled only by the §15.4 setting, failures logged. */
	private async mirrorCache(cacheTask: CacheTask): Promise<void> {
		if (!this.settings.enableCacheMirror) return;
		try {
			await writeCacheFile(this.adapter, cacheTask, formatUtc(new Date()));
		} catch (error) {
			console.error("Taskres: cache mirror failed", error);
		}
	}

	/**
	 * `Taskres: Toggle task done` (§15.4): flips the cursor line's checkbox, adds/removes
	 * `✅ <today>`, on completion moves the line under the Done heading newest-on-top
	 * (creating a level-3 heading at EOF per §6.3), then updates the cache file.
	 */
	private async toggleTaskDone(editor: Editor, file: TFile | null): Promise<void> {
		const lineIdx = editor.getCursor().line;
		const raw = editor.getLine(lineIdx);
		const task = parseLine(raw);
		if (task === undefined) {
			new Notice("Taskres: the cursor is not on a task line.");
			return;
		}

		const doc = editor.getValue();
		const fileParsed = parse(doc, { doneHeading: this.settings.doneHeading });
		const taskIdx = fileParsed.tasks.findIndex((t) => t.lineNo === lineIdx + 1);
		const parent = taskIdx >= 0 ? linkParents(fileParsed.tasks)[taskIdx] : undefined;

		const completing = !task.draft.checked;
		const today = localToday(new Date());
		const newRaw = completing
			? withCompletedToken(flipCheck(raw, true), today)
			: withoutCompletedToken(flipCheck(raw, false));

		// Line surgery over doc.split("\n"): join("\n") reconstructs the document exactly.
		const lines = doc.split("\n");
		let trailingNewline = false;
		let cursorLine: number;

		if (completing) {
			lines.splice(lineIdx, 1);
			let doneIdx = fileParsed.doneHeadingLine !== undefined ? fileParsed.doneHeadingLine - 1 : undefined;
			if (doneIdx !== undefined && doneIdx > lineIdx) doneIdx -= 1;
			if (doneIdx !== undefined) {
				lines.splice(doneIdx + 1, 0, newRaw);
				cursorLine = doneIdx + 1;
			} else {
				if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
				lines.push("", `### ${this.settings.doneHeading}`, newRaw);
				cursorLine = lines.length - 1;
				trailingNewline = true;
			}
		} else {
			lines[lineIdx] = newRaw;
			cursorLine = lineIdx;
		}

		editor.setValue(lines.join("\n") + (trailingNewline ? "\n" : ""));
		editor.setCursor({ line: cursorLine, ch: 0 });

		const newDraft = parseLine(newRaw)?.draft;
		if (newDraft !== undefined && newDraft.uid !== undefined && file !== null) {
			await this.mirrorCache(this.cacheTaskFor(newDraft, newDraft.uid, parent, file, completing ? today : undefined));
		}
	}

	/** `Taskres: Sync now` (§15.4): best-effort cache refresh of the visible file's tasks. */
	private async syncNow(): Promise<void> {
		if (!this.settings.enableCacheMirror) return;
		const view = this.app.workspace.getActiveViewOfType(MarkdownView);
		if (view === null || view.file === null) return;
		const file = view.file;
		const parsed = parse(view.editor.getValue(), { doneHeading: this.settings.doneHeading });
		const parents = linkParents(parsed.tasks);
		let refreshed = 0;
		for (let i = 0; i < parsed.tasks.length; i++) {
			const draft = parsed.tasks[i].draft;
			if (draft.uid === undefined) continue;
			try {
				await writeCacheFile(
					this.adapter,
					this.cacheTaskFor(draft, draft.uid, parents[i], file, draft.completedOn),
					formatUtc(new Date()),
				);
				refreshed += 1;
			} catch (error) {
				console.error("Taskres: cache refresh failed", error);
			}
		}
		new Notice(`Taskres: refreshed ${refreshed} cache ${refreshed === 1 ? "entry" : "entries"}.`);
	}

	private cacheTaskFor(
		draft: TaskDraft,
		uid: string,
		parent: string | undefined,
		file: TFile,
		completedOn: string | undefined,
	): CacheTask {
		return {
			uid: uid,
			text: draft.text,
			done: draft.checked,
			completedOn: completedOn,
			priority: draft.priority,
			due: draft.due,
			start: draft.start,
			scheduled: draft.scheduled,
			created: draft.created,
			parent: parent,
			sourcePath: file.path,
		};
	}
}
