/**
 * restask Obsidian plugin (docs/spec/integrations.md §15): commands, suggestions and
 * settings.
 *
 * This file isolates every Obsidian API call; markdown.ts, modal.ts and toggle.ts stay
 * API-free and unit-testable. The plugin only edits Markdown — on the desktop and on
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
	Notice,
	Plugin,
	SuggestModal,
} from "obsidian";
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
		this.setPlaceholder("high, medium, low, due, start, scheduled, repeat, today, tomorrow…");
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

/** The restask plugin (§15.4). */
export default class RestaskPlugin extends Plugin {
	settings: RestaskSettings = DEFAULT_SETTINGS;

	async onload(): Promise<void> {
		await this.loadSettings();
		this.addSettingTab(new RestaskSettingTab(this.app, this));
		this.registerEditorSuggest(new MetadataSuggest(this.app, this));

		this.addCommand({
			id: "toggle-task-done",
			name: "Toggle task done",
			editorCallback: (editor) => this.toggleTaskDone(editor),
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

	private async loadSettings(): Promise<void> {
		this.settings = Object.assign({}, DEFAULT_SETTINGS, await this.loadData());
	}

	/**
	 * `Toggle task done` (§15.3): completes or reopens the task on the cursor line, in a
	 * note or in TODO.md alike (the daemon carries a TODO.md toggle to the source note).
	 */
	private toggleTaskDone(editor: Editor): void {
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
