//! §15.8: the cursor skips the frontmatter and the headings of the TODO.md view, and the
//! view opens under `# TODO`. The rules (`lockedLines`, `freeLine`, `homeLine`) are pure;
//! the editor side runs against a real CodeMirror `EditorState`. The cases are those of
//! the Neovim test `lock_test.lua`: the two must agree.

import { EditorSelection, EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { viewLock, type FilingHost } from "../src/editor";
import { freeLine, homeLine, lockedLines } from "../src/filing";

// Rows 1-based as in the Lua test; the functions take 0-based indexes.
const VIEW = [
	"---",
	"restask-list: dev",
	"restask-render: 4f461dcfffcd3738",
	"---",
	"# TODO",
	"",
	"## 🔺 Highest Priority",
	"- [ ] a",
	"- [ ] b",
	"",
	"## Done",
	"- [x] c",
];
const row = (n: number): number => n - 1;

describe("§15.8 the locked lines of the view", () => {
	it("are the frontmatter and the headings", () => {
		const locked = lockedLines(VIEW);
		expect([...locked].map((i) => i + 1)).toEqual([1, 2, 3, 4, 5, 7, 11]);
	});

	it("do not include a line of a fenced block", () => {
		expect(lockedLines(["# TODO", "", "```", "# not a heading", "```", "- [ ] a"]).has(3)).toBe(false);
	});

	it("give the view a home under # TODO", () => {
		expect(homeLine(VIEW)).toBe(row(6));
		expect(homeLine(["# TODO"])).toBeUndefined();
		expect(homeLine(["- [ ] a"])).toBeUndefined();
		expect(homeLine(["# TODO", "", "## Done"])).toBe(1);
	});
});

describe("§15.8 where the cursor goes", () => {
	it("stays on a free line", () => expect(freeLine(VIEW, row(8), row(9))).toBe(row(8)));
	it("goes down over a heading", () => expect(freeLine(VIEW, row(7), row(6))).toBe(row(8)));
	it("goes up over a heading", () => expect(freeLine(VIEW, row(7), row(8))).toBe(row(6)));
	it("goes down over the done heading", () => expect(freeLine(VIEW, row(11), row(10))).toBe(row(12)));
	it("stays on the first free line at the top", () => expect(freeLine(VIEW, row(5), row(6))).toBe(row(6)));
	it("lands on the first free line after a jump to the top", () => expect(freeLine(VIEW, row(1), row(8))).toBe(row(6)));
	it("reads an unknown start as down", () => expect(freeLine(VIEW, row(7))).toBe(row(8)));
	it("has no line when every line is locked", () => expect(freeLine(["# A", "## B"], 0)).toBeUndefined());
});

const HOST = { isInboxView: () => true } as unknown as FilingHost;
const text = VIEW.join("\n");

function at(n: number, extra = 0): number {
	return text.split("\n").slice(0, n - 1).join("\n").length + (n > 1 ? 1 : 0) + extra;
}

function moved(from: number, to: number, host: FilingHost = HOST, event = "select"): number {
	const state = EditorState.create({ doc: text, selection: EditorSelection.cursor(from), extensions: [viewLock(host)] });
	const next = state.update({ selection: EditorSelection.cursor(to), userEvent: event }).state;
	return next.doc.lineAt(next.selection.main.head).number;
}

describe("§15.8 the editor extension", () => {
	it("moves a cursor sent onto a heading on to the next line", () => {
		expect(moved(at(6), at(7))).toBe(8);
		expect(moved(at(8), at(7))).toBe(6);
	});

	it("leaves a click alone", () => expect(moved(at(6), at(7), HOST, "select.pointer")).toBe(7));

	it("leaves a note that is not the view alone", () => {
		expect(moved(at(6), at(7), { isInboxView: () => false } as unknown as FilingHost)).toBe(7);
	});

	it("leaves a selection alone", () => {
		const state = EditorState.create({ doc: text, selection: EditorSelection.cursor(at(6)), extensions: [viewLock(HOST)] });
		const next = state.update({ selection: EditorSelection.range(at(6), at(7, 2)) }).state;
		expect(next.selection.main.empty).toBe(false);
	});
});
