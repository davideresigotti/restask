//! §15.6 in the editor: the transaction that registers and files one line, against a real
//! CodeMirror `EditorState` with the §15.5 guard installed, as in Obsidian. The cursor
//! is where it is after the user left the line — on another line, where it has to stay.

import { EditorSelection, EditorState } from "@codemirror/state";
import { describe, expect, it } from "vitest";
import { arrived, carriedSpec, filingOf, restoredSpec, sealSpec, sealedView, uidGuard, type Carried, type FilingHost } from "../src/editor";
import { digest, isSealed } from "../src/filing";

const TODAY = "2026-10-02";
const U = "restask-01jzq4tsvg2c9xkw7n5m8rhdpa";
const A = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const VIEW = "---\nrestask-list: dev\n---\n# TODO\n\n";
const NOTE = "---\nrestask-list: Home\n---\n# Home\n";

const HOST: FilingHost = {
	enabled: () => true,
	startsTasks: () => true,
	// The view is the file at the inbox path; here, the documents made from `VIEW`.
	isInboxView: (state) => state.doc.sliceString(0, 22) === "---\nrestask-list: dev\n",
	inheritsList: () => false,
	doneHeading: () => "Done",
	today: () => TODAY,
	uid: () => U,
	carryMirror: () => Promise.resolve("failed"),
	dropMirror: () => Promise.resolve("failed"),
	mirrorReturned: () => undefined,
	noteTaskSettled: () => undefined,
};

/** A state from text in which `|` marks the cursor; `→` (the line to settle) is dropped. */
function editor(marked: string): EditorState {
	const text = marked.replace("→", "");
	return EditorState.create({ doc: text.replace("|", ""), selection: EditorSelection.single(text.indexOf("|")), extensions: [uidGuard] });
}

/** The state back as text, cursor marked. */
function show(state: EditorState): string {
	const at = state.selection.main.head;
	const text = state.doc.toString();
	return `${text.slice(0, at)}|${text.slice(at)}`;
}

/** Files the line marked `→` in a document whose cursor is at `|`; returns the result, cursor marked. */
function file(marked: string, host: FilingHost = HOST): string | undefined {
	const state = editor(marked);
	const spec = filingOf(state, marked.replace("|", "").indexOf("→"), host)?.spec;
	return spec === undefined ? undefined : show(state.update(spec).state);
}

describe("§15.6 registering where the task was typed", () => {
	it("appends the tokens and leaves the cursor on its line", () => {
		expect(file(`${NOTE}→- [ ] buy milk 🔺\n- [ ] |`)).toBe(`${NOTE}- [ ] buy milk 🔺 🆔 ${U}\n- [ ] |`);
		expect(file(`${NOTE}|- [ ] first${A}\n→- [ ] buy milk  `)).toBe(`${NOTE}|- [ ] first${A}\n- [ ] buy milk 🆔 ${U}`);
	});

	it("keeps a cursor that rests at the end of the line in front of the hidden token", () => {
		expect(file(`${NOTE}→- [ ] buy milk|`)).toBe(`${NOTE}- [ ] buy milk| 🆔 ${U}`);
	});

	it("leaves a registered line, an empty task and prose alone", () => {
		expect(file(`${NOTE}→- [ ] a${A}\n|`)).toBeUndefined();
		expect(file(`${NOTE}→- [ ] \n|`)).toBeUndefined();
		expect(file(`${NOTE}→prose\n|`)).toBeUndefined();
	});
});

describe("§15.6 only routed notes, only where the setting allows", () => {
	it("never touches a note that is local-only (invariant 4)", () => {
		expect(file("# Journal\n→- [ ] private\n|")).toBeUndefined();
		expect(file("---\ntags: x\n---\n→- [ ] private\n|")).toBeUndefined();
	});

	it("registers in a note routed by a folder root", () => {
		expect(file("# Security\n→- [ ] patch\n|", { ...HOST, inheritsList: () => true })).toBe(
			`# Security\n- [ ] patch 🆔 ${U}\n|`,
		);
	});

	it("does nothing on a device that leaves registration to the daemon", () => {
		expect(file(`${NOTE}→- [ ] buy milk\n|`, { ...HOST, enabled: () => false })).toBeUndefined();
	});
});

describe("§15.6 filing in the TODO.md view", () => {
	it("moves the line up to its section; the cursor stays on the line it went to", () => {
		expect(file(`${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n\n## Done\n→- [ ] new 🔺\n- [ ] |\n`)).toBe(
			`${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n- [ ] new 🔺 🆔 ${U}\n\n## Done\n- [ ] |\n`,
		);
	});

	it("moves the line down to its section; the cursor stays on the line it went to", () => {
		expect(file(`${VIEW}## 🔺 Highest Priority\n→- [ ] new 🔽\n- [ ] |\n\n## 🔽 Low Priority\n- [ ] old 🔽${A}\n\n## Done\n`)).toBe(
			`${VIEW}## 🔺 Highest Priority\n- [ ] |\n\n## 🔽 Low Priority\n- [ ] old 🔽${A}\n- [ ] new 🔽 🆔 ${U}\n\n## Done\n`,
		);
	});

	it("creates the section, from the empty view the daemon renders", () => {
		expect(file(`${VIEW}\n## Done\n→- [ ] new 🔺\n|`)).toBe(
			`${VIEW}\n## 🔺 Highest Priority\n- [ ] new 🔺 🆔 ${U}\n\n## Done\n|`,
		);
	});

	it("moves a last line that has no line break after it", () => {
		expect(file(`|${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n\n## Done\n→- [ ] new 🔺`)).toBe(
			`|${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n- [ ] new 🔺 🆔 ${U}\n\n## Done`,
		);
		expect(file(`|${VIEW}→- [ ] new 🔽`)).toBe(`|${VIEW}## 🔽 Low Priority\n- [ ] new 🔽 🆔 ${U}\n`);
	});

	it("moves a registered line whose priority was changed, token and all", () => {
		expect(file(`${VIEW}## No Priority\n→- [ ] now urgent 🔺${A}\n\n## Done\n|`)).toBe(
			`${VIEW}## 🔺 Highest Priority\n- [ ] now urgent 🔺${A}\n\n## Done\n|`,
		);
	});

	it("gives a line typed in a priority section that priority (§7.4)", () => {
		expect(file(`${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n→- [ ] new\n- [ ] |\n\n## Done\n`)).toBe(
			`${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n- [ ] new 🔺 🆔 ${U}\n- [ ] |\n\n## Done\n`,
		);
	});

	it("tells a line that arrived from one that was edited where it stood (§7.4)", () => {
		expect(arrived(`- [ ] a 🔺${A}`, undefined)).toBe(true);
		expect(arrived(`- [ ] a 🔺${A}`, "")).toBe(true);
		expect(arrived(`- [ ] a 🔺${A}`, "- [ ] another 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc")).toBe(true);
		expect(arrived(`- [ ] a 🔺${A}`, `- [ ] a 🔽${A}`)).toBe(false);
		expect(arrived("- [ ] new", undefined)).toBe(false);
		expect(arrived("prose", undefined)).toBe(false);
	});

	it("gives a pasted line the priority of the section it was pasted in, the cursor stays put (§7.4)", () => {
		const marked = `${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n→- [ ] plain 🔽 🆔 ${U}\n|\n## Done\n`;
		const state = editor(marked);
		const at = marked.replace("|", "").indexOf("→");
		const result = filingOf(state, at, HOST, true);
		expect(result?.spec && show(state.update(result.spec).state)).toBe(
			`${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n- [ ] plain 🔺 🆔 ${U}\n|\n## Done\n`,
		);
		// Edited where it stood, the same line is filed by its emoji.
		const edited = filingOf(state, at, HOST)?.spec;
		expect(edited && state.update(edited).state.doc.toString()).toBe(
			`${VIEW}## 🔺 Highest Priority\n- [ ] old 🔺${A}\n\n## 🔽 Low Priority\n- [ ] plain 🔽 🆔 ${U}\n\n## Done\n`,
		);
	});

	it("hands a moved mirror line's new priority over to be carried to its note (§7.4)", () => {
		const marked = `|${VIEW}## 🔽 Low Priority\n→- [ ] theirs 🔺 [[Home#TODO|Home]]${A}\n\n## Done\n`;
		const result = filingOf(editor(marked), marked.replace("|", "").indexOf("→"), HOST, true);
		expect(result).toMatchObject({
			mirror: true,
			was: `- [ ] theirs 🔺 [[Home#TODO|Home]]${A}`,
			now: `- [ ] theirs 🔽 [[Home#TODO|Home]]${A}`,
		});
	});

	it("registers a line that is in its section without moving it", () => {
		expect(file(`${VIEW}## No Priority\n→- [ ] new\n- [ ] |\n\n## Done\n`)).toBe(
			`${VIEW}## No Priority\n- [ ] new 🆔 ${U}\n- [ ] |\n\n## Done\n`,
		);
	});
});

describe("§15.6 the checkbox, in the editor", () => {
	it("completes a task in a note: stamped and moved under the done heading, the cursor stays put", () => {
		expect(file(`${NOTE}→- [x] a${A}\n- [ ] b|\n\n## Done\n`)).toBe(`${NOTE}- [ ] b|\n\n## Done\n- [x] a ✅ ${TODAY}${A}\n`);
	});

	it("reopens a task in a note", () => {
		expect(file(`${NOTE}- [ ] b|\n\n## Done\n→- [ ] a ✅ 2026-09-30${A}\n`)).toBe(`${NOTE}- [ ] b|\n- [ ] a${A}\n\n## Done\n`);
	});

	it("completes a task of the view's own under Done", () => {
		expect(file(`|${VIEW}## No Priority\n→- [x] mine${A}\n\n## Done\n`)).toBe(`|${VIEW}## Done\n- [x] mine ✅ ${TODAY}${A}\n`);
	});

	it("says what else is to be done: a mirror line to carry, a note task whose mirror line may be due", () => {
		const mirror = `|${VIEW}## 🔺 Highest Priority\n→- [x] theirs 🔺 [[Home#TODO|Home]]${A}\n\n## Done\n`;
		const carried = filingOf(editor(mirror), mirror.replace("|", "").indexOf("→"), HOST);
		expect(carried).toEqual({
			spec: undefined,
			inboxView: true,
			uid: A.slice(4),
			mirror: true,
			now: `- [x] theirs 🔺 [[Home#TODO|Home]]${A}`,
		});

		const note = `${NOTE}→- [ ] a 🔺${A}\n|`;
		expect(filingOf(editor(note), note.indexOf("→"), HOST)).toEqual({ spec: undefined, inboxView: false, uid: A.slice(4), mirror: false, now: undefined });
	});
});

describe("§7.1 a mirror line whose task was completed in its note", () => {
	// The cursor is at the very start: the first `|` of the text, before the one in the wikilink.
	const mirror = `|${VIEW}## 🔺 Highest Priority\n- [x] theirs 🔺 [[Home#TODO|Home]]${A}\n- [ ] other 🔺\n\n## Done\n`;
	const after = (marked: string, outcome: Carried): string | undefined => {
		const state = editor(marked);
		const spec = carriedSpec(state, A.slice(4), outcome, HOST);
		return spec === undefined ? undefined : show(state.update(spec).state);
	};

	it("leaves the view", () => {
		expect(after(mirror, "carried")).toBe(`|${VIEW}## 🔺 Highest Priority\n- [ ] other 🔺\n\n## Done\n`);
	});

	it("stays, checked, when the note could not be edited: the daemon carries it later", () => {
		expect(after(mirror, "failed")).toBeUndefined();
	});

	it("is a task of the view's own when no note has it", () => {
		expect(after(mirror, "unclaimed")).toBe(
			`|${VIEW}## 🔺 Highest Priority\n- [ ] other 🔺\n\n## Done\n- [x] theirs 🔺 [[Home#TODO|Home]] ✅ ${TODAY}${A}\n`,
		);
	});

	it("stays when it is still a line the render shows", () => {
		expect(after(mirror.replace("[x] theirs", "[ ] theirs"), "carried")).toBeUndefined();
		expect(after(mirror.replace("[x] theirs", "[ ] theirs"), "unclaimed")).toBeUndefined();
	});

	it("leaves the view when the user took its priority away", () => {
		expect(after(mirror.replace("[x] theirs 🔺", "[ ] theirs"), "carried")).toBe(`|${VIEW}## 🔺 Highest Priority\n- [ ] other 🔺\n\n## Done\n`);
	});

	it("is taken out with the seal claiming no render, so that the missing line is not read as the user's deletion", () => {
		const sealedView = VIEW.replace("restask-list: dev\n", "restask-list: dev\nrestask-render: 0123456789abcdef\n");
		const state = editor(mirror.replace(VIEW, sealedView));
		const spec = carriedSpec(state, A.slice(4), "carried", HOST);
		expect(spec).toBeDefined();
		if (spec === undefined) return;
		expect(state.update(spec).state.doc.toString()).toBe(
			`${sealedView.replace("0123456789abcdef", "0000000000000000")}## 🔺 Highest Priority\n- [ ] other 🔺\n\n## Done\n`,
		);
	});

	it("comes back when the user deleted it and the note has changed the task since: the note wins", () => {
		const state = editor(`|${VIEW}## Done\n`);
		const spec = restoredSpec(state, A.slice(4), `- [ ] theirs 🔺 [[Home#TODO|Home]]${A}`, HOST);
		expect(spec).toBeDefined();
		if (spec === undefined) return;
		expect(show(state.update(spec).state)).toBe(`|${VIEW}## 🔺 Highest Priority\n- [ ] theirs 🔺 [[Home#TODO|Home]]${A}\n\n## Done\n`);
		expect(restoredSpec(state.update(spec).state, A.slice(4), `- [ ] theirs 🔺 [[Home#TODO|Home]]${A}`, HOST)).toBeUndefined();
	});
});

describe("§7.2 the seal, in the editor", () => {
	const head = "---\nrestask-list: dev\n";

	it("is renewed in place after the view was edited, and left alone when right or absent", () => {
		const state = editor(`|${head}restask-render: 0000000000000000\n---\n# TODO\n\n## No Priority\n- [ ] mine${A}\n\n## Done\n`);
		const spec = sealSpec(state, HOST);
		expect(spec).toBeDefined();
		if (spec === undefined) return;
		const sealed = state.update(spec).state;
		expect(isSealed(sealed.doc.toJSON())).toBe(true);
		expect(sealed.doc.lines).toBe(state.doc.lines);
		expect(sealed.doc.toString()).not.toContain("<!--");
		expect(sealSpec(sealed, HOST)).toBeUndefined();
		expect(sealSpec(editor(`|${VIEW}## Done\n`), HOST)).toBeUndefined();
	});
});

describe("§7 the view is the file at the inbox path, whatever a note contains", () => {
	const OLD_DISCLAIMER = "<!-- AUTOGENERATED BY Restask -->";

	it("settles a note that carries a line of a view as a note: registered in place, mirrored into TODO.md", () => {
		// The owner's sandbox note: the disclaimer of an earlier TODO.md right under its frontmatter.
		const note = `---\nrestask-list: Home\n---\n${OLD_DISCLAIMER}\n# Home\n\n# TODO\n`;
		const state = editor(`${note}→- [ ] buy a camera 🔺\n|`);
		const result = filingOf(state, state.doc.toString().indexOf("- [ ]"), HOST);
		expect(result?.inboxView).toBe(false);
		expect(result?.uid).toBe(U);
		expect(result?.spec).toBeDefined();
		if (result?.spec === undefined) return;
		// No section of the view is made in the note; the line stays where it was typed.
		expect(show(state.update(result.spec).state)).toBe(`${note}- [ ] buy a camera 🔺 🆔 ${U}\n|`);
	});

	it("files a line in the inbox file even when nothing in it says it is a view", () => {
		const state = editor(`${VIEW}## Done\n→- [ ] buy milk\n|`);
		const result = filingOf(state, state.doc.toString().indexOf("- [ ]"), HOST);
		expect(result?.inboxView).toBe(true);
		if (result?.spec === undefined) return;
		expect(show(state.update(result.spec).state)).toContain(`## No Priority\n- [ ] buy milk 🆔 ${U}\n`);
	});
});

describe("§7.6 the view a root note holds, in the editor", () => {
	const ROOT = "---\nrestask-list-root: homelab\nrestask-render: 0123456789abcdef\n---\n# Notes\nprose\n# TODO\n\n";
	const theirs = `- [ ] theirs 🔺 [[Networking#TODO|Networking]]${A}`;

	it("files a line typed in the view like a line of TODO.md, and leaves the rest of the note alone", () => {
		expect(file(`|${ROOT}## 🔺 Highest Priority\n${theirs}\n→- [ ] order the cables\n\n## Done\n`)).toBe(
			`|${ROOT}## 🔺 Highest Priority\n${theirs}\n- [ ] order the cables 🔺 🆔 ${U}\n\n## Done\n`,
		);
		expect(file(`|${ROOT}→- [ ] loose\n## Done\n`)).toBe(`|${ROOT}## No Priority\n- [ ] loose 🆔 ${U}\n\n## Done\n`);
		// Above the view the note is a note: registered where it is.
		expect(file(`|${ROOT.replace("prose\n", "→- [ ] above\n")}## Done\n`)).toBe(`|${ROOT.replace("prose\n", `- [ ] above 🆔 ${U}\n`)}## Done\n`);
	});

	it("tells a mirror line from a line of the note's own", () => {
		const state = editor(`|${ROOT}## 🔺 Highest Priority\n${theirs.replace("[ ]", "[x]")}\n- [x] mine 🔺 🆔 ${U}\n\n## Done\n`);
		const line = (text: string): number => state.doc.toString().indexOf(text);
		// A ticked mirror line is a completion to carry to its note; the document waits for the answer.
		expect(filingOf(state, line("- [x] theirs"), HOST)).toMatchObject({ mirror: true, inboxView: false, uid: A.slice(4) });
		expect(filingOf(state, line("- [x] theirs"), HOST)?.spec).toBeUndefined();
		// A ticked line of the note's own is stamped and filed under the view's done heading.
		const mine = filingOf(state, line("- [x] mine"), HOST);
		expect(mine).toMatchObject({ mirror: false, inboxView: false, uid: U });
		expect(mine?.spec === undefined ? undefined : state.update(mine.spec).state.doc.toString()).toBe(
			`${ROOT}## 🔺 Highest Priority\n${theirs.replace("[ ]", "[x]")}\n\n## Done\n- [x] mine 🔺 ✅ ${TODAY} 🆔 ${U}\n`,
		);
	});

	it("takes a carried mirror line out of the view, the seal claiming no render", () => {
		const state = editor(`|${ROOT}## 🔺 Highest Priority\n${theirs.replace("[ ]", "[x]")}\n\n## Done\n`);
		const spec = carriedSpec(state, A.slice(4), "carried", HOST);
		expect(spec).toBeDefined();
		if (spec === undefined) return;
		expect(state.update(spec).state.doc.toString()).toBe(`${ROOT.replace("0123456789abcdef", "0000000000000000")}## Done\n`);
		// A line with that UID outside the view is the note's business, not the view's.
		expect(carriedSpec(editor(`|${ROOT.replace("prose\n", `${theirs.replace("[ ]", "[x]")}\n`)}## Done\n`), A.slice(4), "carried", HOST)).toBeUndefined();
	});

	it("puts a deleted mirror line back when the note wins, and seals the section alone", () => {
		const state = editor(`|${ROOT}## Done\n\n# Links\nmore\n`);
		const spec = restoredSpec(state, A.slice(4), theirs, HOST);
		expect(spec).toBeDefined();
		if (spec === undefined) return;
		const restored = state.update(spec).state;
		expect(restored.doc.toString()).toBe(`${ROOT}## 🔺 Highest Priority\n${theirs}\n\n## Done\n\n# Links\nmore\n`);
		expect(sealedView(restored, HOST)).toBe(false);
		const seal = sealSpec(restored, HOST);
		expect(seal).toBeDefined();
		if (seal === undefined) return;
		const sealed = restored.update(seal).state;
		expect(sealedView(sealed, HOST)).toBe(true);
		expect(sealed.doc.line(3).text).toBe(`restask-render: ${digest(`# TODO\n\n## 🔺 Highest Priority\n${theirs}\n\n## Done\n\n`)}`);
		expect(sealSpec(sealed, HOST)).toBeUndefined();
		// Prose typed outside the view leaves it sealed.
		const typed = sealed.update({ changes: { from: sealed.doc.length, insert: "and more\n" } }).state;
		expect(sealedView(typed, HOST)).toBe(true);
		// A note without a view has no seal to keep.
		expect(sealSpec(editor(`|${NOTE}- [ ] x\n`), HOST)).toBeUndefined();
		expect(sealedView(editor(`|${NOTE}- [ ] x\n`), HOST)).toBe(false);
	});
});
