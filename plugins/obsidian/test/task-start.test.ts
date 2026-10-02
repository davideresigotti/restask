//! §15.7: a new line in a TODO section starts with a checkbox. The rule (`inTodoSection`)
//! is pure; the editor side runs against a real CodeMirror `EditorState` with the §15.5
//! guard installed, as in Obsidian, and is fed the transactions Obsidian makes for Enter.

import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { taskStart, uidGuard, type FilingHost } from "../src/editor";
import { TASK_START, inTodoSection, type NoteKind } from "../src/filing";

const A = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const VIEW = "---\nrestask-list: dev\nrestask-render: 0000000000000000\n---\n# TODO\n\n";
const NOTE = "---\nrestask-list: Home\n---\n# Home\n// Notes ...\n\n";

const IN_NOTE: NoteKind = { inboxView: false, doneHeading: "Done" };
const IN_VIEW: NoteKind = { inboxView: true, doneHeading: "Done" };

/** For each line of `text`, whether it lies in the TODO section: `+` or `-`. */
function sections(text: string, note: NoteKind): string {
	const lines = text.split("\n");
	return lines.map((_line, idx) => (inTodoSection(lines, idx, note) ? "+" : "-")).join("");
}

describe("§15.7 the TODO section of a note", () => {
	it("runs from below the TODO heading to the done heading", () => {
		expect(sections("---\nrestask-list: Home\n---\n# Home\nprose\n\n# TODO\n- [ ] a\n\n## Done\n- [x] b\n", IN_NOTE)).toBe(
			"-------+++--",
		);
	});

	it("ends at the next heading of the same or a higher level, not at a deeper one", () => {
		expect(sections("## TODO\n\n### Later\n\n## Notes\n\n# Top\n", IN_NOTE)).toBe("-++++---");
		expect(sections("## TODO\n\n# Top\n\n", IN_NOTE)).toBe("-++--");
	});

	it("takes the heading in any letter case, and no other text", () => {
		expect(sections("# Todo\n\n", IN_NOTE)).toBe("-++");
		expect(sections("# todo ##\n\n", IN_NOTE)).toBe("-++");
		expect(sections("# TODO list\n\n", IN_NOTE)).toBe("---");
		expect(sections("TODO\n\n", IN_NOTE)).toBe("---");
	});

	it("opens again at a second TODO heading, but never below the done heading", () => {
		expect(sections("# TODO\n\n# Notes\n\n# TODO\n\n", IN_NOTE)).toBe("-++--++");
		expect(sections("# TODO\n\n# Done\n\n# TODO\n\n", IN_NOTE)).toBe("-++----");
		expect(sections("# TODO\n\n# Finished\n\n", { inboxView: false, doneHeading: "Finished" })).toBe("-++--");
	});

	it("leaves out the frontmatter and fenced blocks, and reads no heading in them", () => {
		expect(sections("---\ntitle: TODO\n---\n\n# TODO\n", IN_NOTE)).toBe("-----+");
		expect(sections("# TODO\n```\n# Notes\n\n```\n\n", IN_NOTE)).toBe("-+---++");
		expect(sections("```\n# TODO\n```\n\n", IN_NOTE)).toBe("-----");
	});

	it("is nowhere in a note without the heading", () => {
		expect(sections("# Home\n- [ ] a\n\n", IN_NOTE)).toBe("----");
	});
});

describe("§15.7 the TODO section of the view", () => {
	it("is the whole body above Done", () => {
		expect(sections(`${VIEW}## 🔺 Highest Priority\n- [ ] a 🔺${A}\n\n## No Priority\n\n## Done\n- [x] b\n`, IN_VIEW)).toBe(
			"----++++++++--",
		);
	});

	it("does not depend on the title, nor on the vault's done heading", () => {
		expect(sections("---\nrestask-list: dev\n---\n# Inbox\n\n## Done\n\n", { inboxView: true, doneHeading: "Finished" })).toBe(
			"---+++--",
		);
	});
});

const HOST: FilingHost = {
	enabled: () => false,
	startsTasks: () => true,
	// The view is the file at the inbox path; here, the documents made from `VIEW`.
	isInboxView: (state) => state.doc.sliceString(0, 22) === "---\nrestask-list: dev\n",
	inheritsList: () => false,
	doneHeading: () => "Done",
	today: () => "2026-10-02",
	uid: () => "restask-01jzq4tsvg2c9xkw7n5m8rhdpa",
	carryMirror: () => Promise.resolve("failed"),
	dropMirror: () => Promise.resolve("failed"),
	mirrorReturned: () => undefined,
	noteTaskSettled: () => undefined,
};

/** A state from text in which `|` marks the cursor. */
function editor(marked: string, host: FilingHost = HOST): EditorState {
	return EditorState.create({
		doc: marked.replace("|", ""),
		selection: EditorSelection.single(marked.indexOf("|")),
		extensions: [uidGuard, taskStart(host)],
	});
}

/** The state back as text, cursor marked. */
function show(state: EditorState): string {
	const at = state.selection.main.head;
	const text = state.doc.toString();
	return `${text.slice(0, at)}|${text.slice(at)}`;
}

/** CodeMirror's `insertNewlineAndIndent`, which Obsidian runs for Enter on a line that is no list item. */
function enter(state: EditorState): TransactionSpec {
	const { from, to } = state.selection.main;
	return { changes: { from, to, insert: "\n" }, selection: EditorSelection.cursor(from + 1), userEvent: "input" };
}

/** Obsidian's `newlineAndIndentContinueMarkdownList` on a list item: the character in front of the cursor is written again, with the break and `next`. */
function obsidianEnter(state: EditorState, next: string): TransactionSpec {
	const head = state.selection.main.head;
	return { changes: { from: head - 1, to: head, insert: `${state.doc.sliceString(head - 1, head)}\n${next}` }, userEvent: "input.type" };
}

/** What Enter leaves of `marked`, cursor marked. */
function entered(marked: string, host: FilingHost = HOST): string {
	const state = editor(marked, host);
	return show(state.update(enter(state)).state);
}

describe("§15.7 a new line in a TODO section starts a task", () => {
	it("after the heading, on a blank line and after prose", () => {
		expect(entered(`${NOTE}# TODO|\n\n## Done\n`)).toBe(`${NOTE}# TODO\n- [ ] |\n\n## Done\n`);
		expect(entered(`${NOTE}# TODO\n|\n\n## Done\n`)).toBe(`${NOTE}# TODO\n\n- [ ] |\n\n## Done\n`);
		expect(entered(`${NOTE}# TODO\nremember|\n## Done\n`)).toBe(`${NOTE}# TODO\nremember\n- [ ] |\n## Done\n`);
	});

	it("at the end of a note that stops in the section", () => {
		expect(entered(`${NOTE}# TODO|`)).toBe(`${NOTE}# TODO\n- [ ] |`);
	});

	it("in the view, under any heading above Done", () => {
		expect(entered(`${VIEW}## 🔺 Highest Priority|\n\n## Done\n`)).toBe(`${VIEW}## 🔺 Highest Priority\n- [ ] |\n\n## Done\n`);
		expect(entered(`${VIEW}|\n## Done\n`)).toBe(`${VIEW}\n- [ ] |\n## Done\n`);
	});

	it("when Shift+Enter or Obsidian's Enter breaks a line that is no list item", () => {
		const state = editor(`${NOTE}# TODO\nremember|\n## Done\n`);
		expect(show(state.update(obsidianEnter(state, "")).state)).toBe(`${NOTE}# TODO\nremember\n- [ ] |\n## Done\n`);
	});

	it("when the break replaces a selection", () => {
		const state = EditorState.create({ doc: "---\nrestask-list: Home\n---\n# TODO\nabc\n", extensions: [uidGuard, taskStart(HOST)] });
		const at = state.doc.toString().indexOf("abc");
		const typed = state.update({ changes: { from: at + 1, to: at + 3, insert: "\n" }, selection: EditorSelection.cursor(at + 2), userEvent: "input" });
		expect(show(typed.state)).toBe("---\nrestask-list: Home\n---\n# TODO\na\n- [ ] |\n");
	});

	it("after a registered task, behind its hidden token", () => {
		expect(entered(`${NOTE}# TODO\n- [ ] a|${A}\n`)).toBe(`${NOTE}# TODO\n- [ ] a${A}\n- [ ] |\n`);
	});

	it("is undone together with the line break", () => {
		const state = editor(`${NOTE}# TODO|\n`);
		const tr = state.update(enter(state));
		expect(tr.changes.invert(state.doc).apply(tr.state.doc).toString()).toBe(state.doc.toString());
		let changes = 0;
		tr.changes.iterChanges(() => changes++);
		expect(changes).toBe(1);
	});
});

describe("§15.7 lines that get no checkbox", () => {
	it("a line Obsidian continued the list on has its own", () => {
		const state = editor(`${NOTE}# TODO\n- [ ] a|\n`);
		expect(show(state.update(obsidianEnter(state, TASK_START)).state)).toBe(`${NOTE}# TODO\n- [ ] a\n- [ ] |\n`);
	});

	it("an indented continuation line (Shift+Enter in a list item) stays one", () => {
		const state = editor(`${NOTE}# TODO\n- [ ] a|\n`);
		expect(show(state.update(obsidianEnter(state, "  ")).state)).toBe(`${NOTE}# TODO\n- [ ] a\n  |\n`);
	});

	it("a break in the middle of a line, or in front of one", () => {
		expect(entered(`${NOTE}# TODO\nre|member\n`)).toBe(`${NOTE}# TODO\nre\n|member\n`);
		expect(entered(`${NOTE}# TODO\n|remember\n`)).toBe(`${NOTE}# TODO\n\n|remember\n`);
	});

	it("outside the section: above the heading, under Done, in the frontmatter", () => {
		expect(entered(`${NOTE}|# TODO\n`)).toBe(`${NOTE}\n|# TODO\n`);
		expect(entered(`---\nrestask-list: Home\n---\n# Home|\n\n# TODO\n`)).toBe(`---\nrestask-list: Home\n---\n# Home\n|\n\n# TODO\n`);
		expect(entered(`${NOTE}# TODO\n\n## Done|\n`)).toBe(`${NOTE}# TODO\n\n## Done\n|\n`);
		expect(entered(`${VIEW}## Done|\n`)).toBe(`${VIEW}## Done\n|\n`);
		expect(entered("---\nrestask-list: Home|\n---\n# TODO\n")).toBe("---\nrestask-list: Home\n|\n---\n# TODO\n");
	});

	it("a note that is local-only (invariant 4), also under a TODO heading", () => {
		expect(entered("# Journal\n# TODO|\n")).toBe("# Journal\n# TODO\n|\n");
		expect(entered("---\ntags: x\n---\n# TODO|\n")).toBe("---\ntags: x\n---\n# TODO\n|\n");
	});

	it("a note routed by a folder root does get one", () => {
		expect(entered("# TODO|\n", { ...HOST, inheritsList: () => true })).toBe("# TODO\n- [ ] |\n");
	});

	it("text that ends in a line break: a paste, a template", () => {
		const state = editor(`${NOTE}# TODO\n|\n`);
		const at = state.selection.main.head;
		const paste = state.update({ changes: { from: at, insert: "\n" }, selection: EditorSelection.cursor(at + 1), userEvent: "input.paste" });
		expect(show(paste.state)).toBe(`${NOTE}# TODO\n\n|\n`);
		const template = state.update({ changes: { from: at, insert: "one\ntwo\n" }, selection: EditorSelection.cursor(at + 8) });
		expect(show(template.state)).toBe(`${NOTE}# TODO\none\ntwo\n|\n`);
	});

	it("a reload from disk, undo and redo", () => {
		const state = editor(`${NOTE}# TODO|\n`);
		for (const userEvent of ["set", "undo", "redo"]) {
			expect(show(state.update({ ...enter(state), userEvent }).state)).toBe(`${NOTE}# TODO\n|\n`);
		}
	});

	it("with the setting off; the setting of §15.6 is another one", () => {
		expect(entered(`${NOTE}# TODO|\n`, { ...HOST, startsTasks: () => false })).toBe(`${NOTE}# TODO\n|\n`);
		expect(entered(`${NOTE}# TODO|\n`, { ...HOST, enabled: () => true })).toBe(`${NOTE}# TODO\n- [ ] |\n`);
	});
});
