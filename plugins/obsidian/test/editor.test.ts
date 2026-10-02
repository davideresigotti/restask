//! §15.5 editing around the hidden `🆔` token: the cursor stays out of it, and no edit
//! deletes it or moves it to another line unless the task's whole body goes too. Run
//! against a real CodeMirror `EditorState`; the transactions have the shapes Obsidian
//! dispatches (its list-continuing Enter replaces the character before the cursor).

import { EditorSelection, EditorState, type TransactionSpec } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { uidGuard } from "../src/editor";

const A = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpa";
const B = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const C = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc";

/** A state from text in which `|` marks the cursor (or the selection head) and `‹` the anchor. */
function editor(marked: string): EditorState {
	const anchor = marked.replace("|", "").indexOf("‹");
	const head = marked.replace("‹", "").indexOf("|");
	return EditorState.create({
		doc: marked.replace("|", "").replace("‹", ""),
		selection: EditorSelection.single(anchor < 0 ? head : anchor, head),
		extensions: [uidGuard],
	});
}

/** The state back as marked text. */
function show(state: EditorState): string {
	const { anchor, head } = state.selection.main;
	const marks = anchor === head ? [[head, "|"]] : [[anchor, "‹"], [head, "|"]];
	let text = state.doc.toString();
	for (const [pos, mark] of marks.sort((x, y) => Number(y[0]) - Number(x[0]))) {
		text = text.slice(0, Number(pos)) + mark + text.slice(Number(pos));
	}
	return text;
}

function apply(marked: string, spec: (state: EditorState) => TransactionSpec): string {
	const state = editor(marked);
	return show(state.update(spec(state)).state);
}

/** Moves the cursor to the position marked `|` in `target` (same text). */
function moveTo(marked: string, target: string): string {
	const head = target.replace("‹", "").indexOf("|");
	const anchor = target.replace("|", "").indexOf("‹");
	return apply(marked, () => ({ selection: EditorSelection.single(anchor < 0 ? head : anchor, head), userEvent: "select" }));
}

/** Types `text` over the selection, as CodeMirror's input handler does. */
function type(marked: string, text: string): string {
	return apply(marked, (state) => ({ ...state.replaceSelection(text), userEvent: "input.type" }));
}

/** Deletes `from..to` given as offsets relative to the cursor. */
function remove(marked: string, before: number, after: number, userEvent: string): string {
	return apply(marked, (state) => {
		const head = state.selection.main.head;
		return { changes: { from: head - before, to: head + after }, userEvent };
	});
}

/** Deletes the selection. */
function removeSelection(marked: string): string {
	return apply(marked, (state) => ({ ...state.replaceSelection(""), userEvent: "delete.selection" }));
}

/** Obsidian's Enter inside a list item: the character before the cursor, a line break, the marker. */
function enter(marked: string, userEvent?: string): string {
	return apply(marked, (state) => {
		const head = state.selection.main.head;
		const insert = `${state.doc.sliceString(head - 1, head)}\n- [ ] `;
		return { changes: { from: head - 1, to: head, insert }, userEvent };
	});
}

describe("§15.5 the cursor stays out of the token", () => {
	it("lands in front of the token when sent behind it (End, a click, arrow up or down)", () => {
		expect(moveTo(`- [ ] |a${A}\n- [ ] b${B}`, `- [ ] a${A}|\n- [ ] b${B}`)).toBe(`- [ ] a|${A}\n- [ ] b${B}`);
		expect(moveTo(`- [ ] a${A}\n- [ ] |b${B}`, `- [ ] a 🆔 rest|ask-01jzq4tsvg2c9xkw7n5m8rhdpa\n- [ ] b${B}`)).toBe(
			`- [ ] a|${A}\n- [ ] b${B}`,
		);
	});

	it("moves left from the next line to the visible end of the line", () => {
		expect(moveTo(`- [ ] a${A}\n|- [ ] b${B}`, `- [ ] a${A}|\n- [ ] b${B}`)).toBe(`- [ ] a|${A}\n- [ ] b${B}`);
	});

	it("moves right from the visible end of the line to the next line", () => {
		expect(moveTo(`- [ ] a|${A}\n- [ ] b${B}`, `- [ ] a |🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpa\n- [ ] b${B}`)).toBe(
			`- [ ] a${A}\n|- [ ] b${B}`,
		);
	});

	it("moves right over the token to the text behind it", () => {
		expect(moveTo(`- [ ] a|${A} tail`, `- [ ] a |🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpa tail`)).toBe(`- [ ] a${A} |tail`);
	});

	it("stays put at the visible end when End is pressed there, and at the end of the document", () => {
		expect(moveTo(`- [ ] a|${A}\nx`, `- [ ] a${A}|\nx`)).toBe(`- [ ] a|${A}\nx`);
		expect(moveTo(`- [ ] a|${A}`, `- [ ] a |🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpa`)).toBe(`- [ ] a|${A}`);
	});

	it("ends a selection made inside the text in front of the token", () => {
		expect(moveTo(`- [ ] he|llo${A}`, `- [ ] he‹llo${A}|`)).toBe(`- [ ] he‹llo|${A}`);
	});

	it("keeps the token in a selection of the whole body, the whole line or several lines", () => {
		expect(moveTo(`- [ ] |a${A}`, `- [ ] ‹a${A}|`)).toBe(`- [ ] ‹a${A}|`);
		expect(moveTo(`|- [ ] a${A}`, `‹- [ ] a${A}|`)).toBe(`‹- [ ] a${A}|`);
		expect(moveTo(`x|y\n- [ ] a${A}`, `x‹y\n- [ ] a${A}|`)).toBe(`x‹y\n- [ ] a${A}|`);
	});

	it("leaves lines without a hidden token alone", () => {
		expect(moveTo(`|prose${A}`, `prose${A}|`)).toBe(`prose${A}|`);
		expect(moveTo("- [ ] |plain", "- [ ] plain|")).toBe("- [ ] plain|");
	});
});

describe("§15.5 typing next to the token", () => {
	it("puts typed text in front of the token", () => {
		expect(type(`- [ ] a|${A}`, " b")).toBe(`- [ ] a b|${A}`);
	});

	it("shows a blank typed at the visible end and keeps the cursor behind it", () => {
		expect(type(`- [ ] a|${A}`, " ")).toBe(`- [ ] a |${A}`);
		expect(type(type(`- [ ] a|${A}`, " "), "🔺")).toBe(`- [ ] a 🔺|${A}`);
		expect(type(type(`- [ ] a|${A}`, " "), " ")).toBe(`- [ ] a  |${A}`);
	});

	it("rests behind the blanks a line was saved with, one of them hidden with the token", () => {
		expect(moveTo(`- [ ] |a  ${A}`, `- [ ] a  ${A}|`)).toBe(`- [ ] a  |${A}`);
		expect(remove(`- [ ] a  |${A}`, 1, 0, "delete.backward")).toBe(`- [ ] a |${A}`);
	});

	it("does so for text inserted behind the token as well", () => {
		expect(apply(`|- [ ] a${A}`, (state) => ({ changes: { from: state.doc.length, insert: " ✅ 2026-10-01" } }))).toBe(
			`|- [ ] a ✅ 2026-10-01${A}`,
		);
	});

	it("types at each of several cursors", () => {
		const next = EditorState.create({
			doc: `- [ ] a${A}\n- [ ] b${B}`,
			selection: EditorSelection.create([EditorSelection.cursor(7), EditorSelection.cursor(7 + A.length + 8)]),
			extensions: [uidGuard, EditorState.allowMultipleSelections.of(true)],
		});
		const typed = next.update({ ...next.replaceSelection("!"), userEvent: "input.type" }).state;
		expect(typed.doc.toString()).toBe(`- [ ] a!${A}\n- [ ] b!${B}`);
		expect(typed.selection.ranges.map((range) => range.head)).toEqual([8, 8 + A.length + 9]);
	});
});

describe("§15.5 a new line never takes the token along", () => {
	it("Enter at the visible end starts an item without a token", () => {
		expect(enter(`- [ ] a|${A}\n- [ ] b${B}`, "input.type")).toBe(`- [ ] a${A}\n- [ ] |\n- [ ] b${B}`);
	});

	it("Shift+Enter and plain line breaks, which carry no user event, do the same", () => {
		expect(enter(`- [ ] a|${A}`)).toBe(`- [ ] a${A}\n- [ ] |`);
		expect(apply(`- [ ] a|${A}`, (state) => state.replaceSelection("\n"))).toBe(`- [ ] a${A}\n|`);
	});

	it("keeps the text behind the token with the new line", () => {
		expect(enter(`- [ ] a|${A} tail`, "input.type")).toBe(`- [ ] a${A}\n- [ ] | tail`);
	});

	it("pastes several lines around the token: the first before it, the rest behind", () => {
		expect(apply(`- [ ] a|${A}`, (state) => ({ ...state.replaceSelection(" x\n- [ ] y"), userEvent: "input.paste" }))).toBe(
			`- [ ] a x${A}\n- [ ] y|`,
		);
	});

	it("splits a line in the middle of its text as text", () => {
		expect(enter(`- [ ] he|llo${A}`, "input.type")).toBe(`- [ ] he\n- [ ] |llo${A}`);
	});

	it("leaves a replacement that starts on an earlier line as it is", () => {
		const spec = { changes: { from: 1, to: 10, insert: "1\n2" } };
		const unguarded = EditorState.create({ doc: `xy\n- [ ] a${A}`, selection: EditorSelection.cursor(1) }).update(spec).state;
		expect(show(unguarded)).toBe(`x|1\n2${A}`);
		// …except for the token, which would show on the line that is no task any more.
		expect(apply(`x|y\n- [ ] a${A}`, () => spec)).toBe("x|1\n2");
	});

	it("leaves a line appended behind the token where it is", () => {
		expect(apply(`|- [ ] a${A}`, (state) => ({ changes: { from: state.doc.length, insert: `\n- [ ] a${B}` } }))).toBe(
			`|- [ ] a${A}\n- [ ] a${B}`,
		);
	});
});

describe("§15.5 deleting next to the token", () => {
	it("Backspace at the visible end deletes the visible character", () => {
		expect(remove(`- [ ] ab|${A}`, 1, 0, "delete.backward")).toBe(`- [ ] a|${A}`);
	});

	it("Backspace that would hit the token takes the character before it instead", () => {
		expect(apply(`- [ ] ab|${A}`, (state) => ({ changes: { from: state.doc.length - 1, to: state.doc.length }, userEvent: "delete.backward" }))).toBe(
			`- [ ] a|${A}`,
		);
		expect(remove(`- [ ] a😀|${A}`, 0, A.length, "delete.backward")).toBe(`- [ ] a|${A}`);
	});

	it("Delete at the visible end does nothing, or takes the text behind the token", () => {
		expect(remove(`- [ ] a|${A}\nnext`, 0, 1, "delete.forward")).toBe(`- [ ] a|${A}\nnext`);
		expect(remove(`- [ ] a|${A} tail`, 0, 1, "delete.forward")).toBe(`- [ ] a|${A}tail`);
	});

	it("deleting to the end of the line keeps the token", () => {
		expect(removeSelection(`- [ ] he‹llo${A}|`)).toBe(`- [ ] he|${A}`);
		expect(remove(`- [ ] he|llo${A}`, 0, 3 + A.length, "delete.forward")).toBe(`- [ ] he|${A}`);
	});

	it("typing over a selection that reaches into the token keeps the token", () => {
		expect(apply(`- [ ] he|llo${A}`, (state) => ({ changes: { from: 8, to: state.doc.length - 3, insert: "y" }, selection: { anchor: 9 }, userEvent: "input.type" }))).toBe(
			`- [ ] hey|${A}`,
		);
	});

	it("an emptied task gives its token up", () => {
		expect(remove(`- [ ] |${A}`, 0, A.length, "delete.backward")).toBe("- [ ] |");
		expect(removeSelection(`- [ ] ‹hello${A}|`)).toBe("- [ ] |");
	});

	it("deleting or cutting whole lines takes their tokens", () => {
		expect(removeSelection(`‹- [ ] a${A}\n|- [ ] b${B}`)).toBe(`|- [ ] b${B}`);
		expect(removeSelection(`- [ ] a${A}‹\n- [ ] b${B}|`)).toBe(`- [ ] a|${A}`);
	});

	it("a deletion across lines keeps the token of the line it starts in, and only that one", () => {
		expect(removeSelection(`- [ ] he‹llo${A}\n- [ ] b${B}\n- [ ] wor|ld${C}`)).toBe(`- [ ] he|${A}ld`);
	});
});

describe("§15.5 a token never comes out of hiding", () => {
	it("goes with the line's text when that is selected from the visible end and deleted", () => {
		expect(removeSelection(`|- [ ] hello‹${A}`)).toBe("|");
		expect(removeSelection(`|- [ ] hello‹${A}\n- [ ] b${B}`)).toBe(`|\n- [ ] b${B}`);
		expect(removeSelection(`  |- [ ] hello  ‹${A}`)).toBe("  |");
	});

	it("goes when the checkbox in front of an emptied task is deleted", () => {
		expect(remove(`- [ ]| ${A}`, 5, 0, "delete.backward")).toBe("|  ");
		expect(remove(`- [ ]|${A}`, 1, 0, "delete.backward")).toBe("- [ | ");
	});

	it("goes when an edit turns the task into an ordinary line", () => {
		expect(removeSelection(`- ‹[ ] |hello${A}`)).toBe("- |hello");
		expect(apply(`|- [ ] hello${A}`, () => ({ changes: { from: 0, to: 6, insert: "> " } }))).toBe("|> hello");
		expect(apply(`- [| ] hello${A}`, () => ({ changes: { from: 3, to: 4, insert: "/" } }))).toBe("- [|/] hello");
		expect(type(`- [ |] hello${A}`, "?")).toBe("- [ ?|] hello");
	});

	it("stays with the head of its task when a plain line break cuts the text off", () => {
		expect(apply(`- [ ] he|llo${A}`, (state) => ({ ...state.replaceSelection("\n"), userEvent: "input.type" }))).toBe(
			`- [ ] he${A}\n|llo`,
		);
		expect(apply(`|- [ ] hello${A}`, () => ({ changes: { from: 8, insert: "\nx\n" } }))).toBe(`|- [ ] he${A}\nx\nllo`);
	});

	it("goes when a deletion from a line that is no task ends inside it", () => {
		expect(apply(`|x\n- [ ] b${B}`, () => ({ changes: { from: 1, to: 14 }, userEvent: "delete.selection" }))).toBe("|x");
		expect(removeSelection(`x‹\n- [ ] |b${B}`)).toBe("x|b");
	});

	it("stays while the line is a task, whatever else changes on it", () => {
		expect(removeSelection(`‹  |- [ ] hello${A}`)).toBe(`|- [ ] hello${A}`);
		expect(apply(`- [| ] hello${A}`, () => ({ changes: { from: 3, to: 4, insert: "x" } }))).toBe(`- [|x] hello${A}`);
		expect(apply(`|- [ ] hello${A}`, () => ({ changes: { from: 0, to: 1, insert: "*" } }))).toBe(`|* [ ] hello${A}`);
	});
});

describe("§15.5 what the guard lets through", () => {
	it("a reload from disk may rewrite a token", () => {
		const reassigned = "restask-01jzq4tsvg2c9xkw7n5m8rhdpz";
		expect(apply(`|- [ ] a${A}`, (state) => ({ changes: { from: state.doc.length - 1, insert: "z", to: state.doc.length }, userEvent: "set" }))).toBe(
			`|- [ ] a 🆔 ${reassigned}`,
		);
	});

	it("undo and redo replay earlier documents", () => {
		expect(apply(`|- [ ] a${A}`, (state) => ({ changes: { from: 7, to: state.doc.length }, userEvent: "undo" }))).toBe("|- [ ] a");
	});

	it("replacing whole lines or the whole document", () => {
		expect(apply(`|- [ ] a${A}\n- [ ] b${B}`, (state) => ({ changes: { from: 0, to: state.doc.length, insert: "- [x] a" } }))).toBe("|- [x] a");
		expect(apply(`|- [ ] a${A}`, (state) => ({ changes: { from: 0, to: state.doc.length, insert: `- [x] a ✅ 2026-10-01${A}` } }))).toBe(
			`|- [x] a ✅ 2026-10-01${A}`,
		);
	});

	it("a token on a line that is not a task is ordinary text", () => {
		expect(remove(`prose${A}|`, 1, 0, "delete.backward")).toBe(`prose${A.slice(0, -1)}|`);
	});
});
