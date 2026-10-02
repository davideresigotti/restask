//! §15.6 registering and filing a new task: the pure line transformations. What they
//! produce has to be what the daemon would have produced (§6.4 registration, §7 render).

import { describe, expect, it } from "vitest";
import {
	canonicalLine,
	completedByUid,
	digest,
	disclaimed,
	isSealed,
	mirrorDropped,
	mirrorEdited,
	mirrorLine,
	mirrorSource,
	mirrored,
	putBack,
	refiled,
	registeredLine,
	removed,
	resealed,
	routesItself,
	settled,
	splices,
	statusRepaired,
	toggledLines,
	uidGenerator,
} from "../src/filing";
import { parseLine } from "../src/markdown";

const TODAY = "2026-10-02";
const U = "restask-01jzq4tsvg2c9xkw7n5m8rhdpa";
const A = "🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const B = "🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc";
const uid = (): string => U;
const NOTE = { inboxView: false, doneHeading: "Done" };
const VIEW = { inboxView: true, doneHeading: "Done" };

/** The note after the line on `idx` was settled; undefined when it does not change. */
function filed(lines: string[], idx: number, inboxView: boolean, today: string, next: () => string): string[] | undefined {
	return settled(lines, idx, inboxView ? VIEW : NOTE, today, next)?.lines;
}

const HEAD = ["---", "restask-list: dev", "---", "# TODO", ""];

function view(...body: string[]): string[] {
	return [...HEAD, ...body];
}

/** Applies splices the way an editor does: all positions refer to the document before. */
function patch(before: string[], after: string[]): string[] {
	const out = [...before];
	for (const s of [...splices(before, after)].reverse()) out.splice(s.at, s.remove, ...s.insert);
	return out;
}

describe("§3.1 UIDs made on the device", () => {
	it("are valid task UIDs the grammar reads back", () => {
		const next = uidGenerator(
			() => Date.UTC(2026, 9, 2),
			(count) => new Uint8Array(count).fill(7),
		);
		const made = next();
		expect(made).toMatch(/^restask-[0-9a-hjkmnp-tv-z]{26}$/);
		expect(parseLine(`- [ ] a 🆔 ${made}`)?.draft.uid).toBe(made);
		// 2026-10-02T00:00:00Z in milliseconds, base32, then 16 times the digit 7.
		expect(made).toBe("restask-01m3wyj8007777777777777777");
	});

	it("uses five bits of each random byte", () => {
		const next = uidGenerator(
			() => 0,
			() => Uint8Array.from([0, 31, 32, 63, 255, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]),
		);
		expect(next()).toBe("restask-00000000000z0zz123456789ab");
	});

	it("keep the order they were made in, also within one millisecond and across a clock step back", () => {
		let now = 1000;
		const next = uidGenerator(
			() => now,
			(count) => new Uint8Array(count).fill(7),
		);
		const first = next();
		const second = next();
		now = 500;
		const third = next();
		now = 2000;
		const fourth = next();
		expect([first, second, third, fourth]).toEqual([...new Set([first, second, third, fourth])].sort());
	});
});

describe("§15.6 registering a line", () => {
	it("appends the UID behind everything else, and no creation date", () => {
		expect(registeredLine("- [ ] buy milk", TODAY, uid)).toBe(`- [ ] buy milk 🆔 ${U}`);
		expect(registeredLine("\t* [ ] buy milk 🔺 📅 2026-10-05  ", TODAY, uid)).toBe(
			`\t* [ ] buy milk 🔺 📅 2026-10-05 🆔 ${U}`,
		);
		expect(registeredLine("- [x] done already", TODAY, uid)).toBe(`- [x] done already 🆔 ${U}`);
	});

	it("gives a line typed in a priority section that priority, unless it names one (§7.4)", () => {
		expect(registeredLine("- [ ] buy milk  ", TODAY, uid, "high")).toBe(`- [ ] buy milk ⏫ 🆔 ${U}`);
		expect(registeredLine("- [ ] buy milk 🔽", TODAY, uid, "high")).toBe(`- [ ] buy milk 🔽 🆔 ${U}`);
		expect(registeredLine(`- [ ] buy milk ${A}`, TODAY, uid, "high")).toBeUndefined();
	});

	it("keeps a creation date the line already has", () => {
		expect(registeredLine("- [ ] a ➕ 2026-09-01", TODAY, uid)).toBe(`- [ ] a ➕ 2026-09-01 🆔 ${U}`);
	});

	it("answers a bare ➕ with today, where it stands (§6.4)", () => {
		expect(registeredLine("- [ ] buy milk ➕", TODAY, uid)).toBe(`- [ ] buy milk ➕ ${TODAY} 🆔 ${U}`);
		expect(registeredLine("- [ ] buy milk ➕ 📅 2026-10-05", TODAY, uid, "high")).toBe(
			`- [ ] buy milk ➕ ${TODAY} 📅 2026-10-05 ⏫ 🆔 ${U}`,
		);
		// The date the line states is the answer already.
		expect(registeredLine("- [ ] a ➕ 2026-09-01 ➕", TODAY, uid)).toBe(`- [ ] a ➕ 2026-09-01 ➕ 🆔 ${U}`);
	});

	it("leaves a bare ➕ on a registered task for the daemon, which knows the server's date (§6.4)", () => {
		const note = ["---", "restask-list: Home", "---", "# Home", `- [ ] old one ➕ ${A}`];
		expect(settled(note, 4, NOTE, TODAY, uid)).toEqual({ lines: undefined, uid: A.slice(3) });
		const draft = parseLine(note[4])?.draft;
		expect(draft?.text).toBe("old one");
		expect(draft && canonicalLine("", "-", { ...draft, priority: "high" })).toBe(`- [ ] old one ⏫ ➕ ${A}`);
	});

	it("leaves alone what is not a new task with text", () => {
		let asked = 0;
		const counting = (): string => {
			asked++;
			return U;
		};
		for (const line of ["prose", "1. [ ] ordered", "-[ ] no space", "- [ ] ", "- [ ] 🔺", `- [ ] a ${A}`, "- [ ] a 🆔 restask-broken"]) {
			expect(registeredLine(line, TODAY, counting)).toBeUndefined();
		}
		expect(asked).toBe(0);
	});
});

describe("§15.6 which notes take part", () => {
	it("reads the routing markers like the engine does (§5.1)", () => {
		expect(routesItself(["---", "restask-list: Home", "---", "- [ ] a"])).toBe(true);
		expect(routesItself(["---", "tags: x", "  restask-list-root :  Home Lab ", "---"])).toBe(true);
		expect(routesItself(["---", "restask-list:", "---"])).toBe(false);
		expect(routesItself(["---", "Restask-List: Home", "---"])).toBe(false);
		expect(routesItself(["---", "restask-list: Home"])).toBe(false);
		expect(routesItself(["", "---", "restask-list: Home", "---"])).toBe(false);
		expect(routesItself(["restask-list: Home"])).toBe(false);
	});
});

describe("§15.6 filing a line in the TODO.md view", () => {
	it("moves a prioritized task behind the last task of its section", () => {
		const before = view("## 🔺 Highest Priority", `- [ ] old 🔺 [[Home#TODO|Home]] ${A}`, "", "## Done", `- [ ] new 🔺 ${B}`, "");
		expect(refiled(before, 9)).toEqual(
			view("## 🔺 Highest Priority", `- [ ] old 🔺 [[Home#TODO|Home]] ${A}`, `- [ ] new 🔺 ${B}`, "", "## Done", ""),
		);
	});

	it("creates a missing section in render order", () => {
		const before = view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, "", "## 🔽 Low Priority", `- [ ] new 🔼 ${B}`, "- [ ] low 🔽", "", "## Done", "");
		expect(refiled(before, 9)).toEqual(
			view(
				"## 🔺 Highest Priority",
				`- [ ] old 🔺 ${A}`,
				"",
				"## 🔼 Medium Priority",
				`- [ ] new 🔼 ${B}`,
				"",
				"## 🔽 Low Priority",
				"- [ ] low 🔽",
				"",
				"## Done",
				"",
			),
		);
	});

	it("puts a task without a priority under No Priority, which comes below the priorities", () => {
		const before = view("## 🔽 Low Priority", `- [ ] old 🔽 ${A}`, "", "## Done", `- [ ] new ${B}`, "");
		expect(refiled(before, 9)).toEqual(
			view("## 🔽 Low Priority", `- [ ] old 🔽 ${A}`, "", "## No Priority", `- [ ] new ${B}`, "", "## Done", ""),
		);
	});

	it("produces the daemon's first render from the empty view", () => {
		// crates/restask/tests/fixtures-style empty view: title, blank, `## Done`.
		const before = view(`- [ ] new 🔺 ➕ ${TODAY} ${B}`, "## Done", "");
		expect(refiled(before, 5)).toEqual(view("## 🔺 Highest Priority", `- [ ] new 🔺 ➕ ${TODAY} ${B}`, "", "## Done", ""));
	});

	it("drops a section the line leaves empty, but never Done", () => {
		const before = view("## No Priority", `- [ ] now urgent 🔺 ${A}`, "", "## Done", "");
		expect(refiled(before, 6)).toEqual(view("## 🔺 Highest Priority", `- [ ] now urgent 🔺 ${A}`, "", "## Done", ""));

		const reopened = view("## Done", `- [ ] back ${A}`, "");
		expect(refiled(reopened, 6)).toEqual(view("## No Priority", `- [ ] back ${A}`, "", "## Done", ""));
	});

	it("keeps a section that still holds anything else", () => {
		const before = view("## No Priority", `- [ ] now urgent 🔺 ${A}`, "a note to self", "", "## Done", "");
		expect(refiled(before, 6)).toEqual(
			view("## 🔺 Highest Priority", `- [ ] now urgent 🔺 ${A}`, "", "## No Priority", "a note to self", "", "## Done", ""),
		);
	});

	it("appends the section when the view has no later one", () => {
		expect(refiled([...HEAD, `- [ ] new 🔽 ${A}`], 5)).toEqual([...HEAD, "## 🔽 Low Priority", `- [ ] new 🔽 ${A}`, ""]);
		expect(refiled(["# TODO", `- [ ] new ${A}`, ""], 1)).toEqual(["# TODO", "", "## No Priority", `- [ ] new ${A}`, ""]);
	});

	it("leaves a line that is where it belongs", () => {
		const doc = view("## 🔺 Highest Priority", `- [ ] b 🔺 ${B}`, "", "## No Priority", `- [ ] a ${A}`, "", "## Done", "- [x] c ✅ 2026-10-01", "");
		for (const idx of [6, 9, 12]) expect(refiled(doc, idx)).toBeUndefined();
	});

	it("never moves a mirror line the render would drop, or a non-task", () => {
		const doc = view(
			"## 🔺 Highest Priority",
			`- [x] done 🔽 [[Home#TODO|Home]] ${A}`,
			`- [ ] mirror [[Home#TODO|Home]] ${B}`,
			"prose",
			"",
			"## Done",
			"",
		);
		for (const idx of [6, 7, 8]) expect(refiled(doc, idx)).toBeUndefined();
	});

	it("moves a completed task of the view's own to Done, newest first, then by UID", () => {
		const older = "- [x] older ✅ 2026-09-30 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpz";
		const sameDay = `- [x] same day ✅ ${TODAY} 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpz`;
		const before = view("## No Priority", `- [x] mine ✅ ${TODAY} ${B}`, "- [ ] other", "", "## Done", sameDay, older, "");
		expect(refiled(before, 6)).toEqual(view("## No Priority", "- [ ] other", "", "## Done", sameDay, `- [x] mine ✅ ${TODAY} ${B}`, older, ""));

		const first = view("## No Priority", `- [x] mine ✅ ${TODAY} ${B}`, "", "## Done", older, "");
		expect(refiled(first, 6)).toEqual(view("## Done", `- [x] mine ✅ ${TODAY} ${B}`, older, ""));
	});

	it("treats a mirror-shaped line as the view's own when told no note has the task", () => {
		const before = view("## 🔺 Highest Priority", `- [x] read 🔺 ✅ ${TODAY} [[Book]] ${A}`, "", "## Done", "");
		expect(refiled(before, 6)).toBeUndefined();
		expect(refiled(before, 6, true)).toEqual(view("## Done", `- [x] read 🔺 ✅ ${TODAY} [[Book]] ${A}`, ""));
	});

	it("takes a line out, with the section it leaves empty", () => {
		const doc = view("## 🔺 Highest Priority", `- [x] a 🔺 [[Home|Home]] ${A}`, "", "## Done", "");
		expect(removed(doc, 6)).toEqual(view("## Done", ""));
		const two = view("## 🔺 Highest Priority", `- [x] a 🔺 [[Home|Home]] ${A}`, `- [ ] b 🔺 ${B}`, "", "## Done", "");
		expect(removed(two, 6)).toEqual(view("## 🔺 Highest Priority", `- [ ] b 🔺 ${B}`, "", "## Done", ""));
	});

	it("moves a mirror line whose priority changed, wherever it goes", () => {
		const before = view("## 🔺 Highest Priority", `- [ ] mirror 🔽 [[Home#TODO|Home]] ${A}`, `- [ ] other 🔺 ${B}`, "", "## Done", "");
		expect(refiled(before, 6)).toEqual(
			view("## 🔺 Highest Priority", `- [ ] other 🔺 ${B}`, "", "## 🔽 Low Priority", `- [ ] mirror 🔽 [[Home#TODO|Home]] ${A}`, "", "## Done", ""),
		);
	});
});

describe("§15.6 registering and filing together", () => {
	it("registers in place in a note", () => {
		const note = ["---", "restask-list: Home", "---", "# Home", "- [ ] a 🔺", "- [ ] b"];
		expect(filed(note, 4, false, TODAY, uid)).toEqual([...note.slice(0, 4), `- [ ] a 🔺 🆔 ${U}`, "- [ ] b"]);
	});

	it("registers and files in the view", () => {
		const before = view("## Done", "- [ ] new 🔺", "");
		expect(filed(before, 6, true, TODAY, uid)).toEqual(
			view("## 🔺 Highest Priority", `- [ ] new 🔺 🆔 ${U}`, "", "## Done", ""),
		);
	});

	it("does nothing to a line that needs nothing", () => {
		const doc = view("## No Priority", `- [ ] a ${A}`, "- [ ] ", "", "## Done", "");
		expect(filed(doc, 6, true, TODAY, uid)).toBeUndefined();
		expect(filed(doc, 7, true, TODAY, uid)).toBeUndefined();
		expect(filed(doc, 8, true, TODAY, uid)).toBeUndefined();
		expect(filed(doc, 97, true, TODAY, uid)).toBeUndefined();
	});

	it("does not take a task-shaped line of the frontmatter or of a code block for a task", () => {
		const note = ["---", "restask-list: Home", "- [ ] not a task", "---", "```", "- [ ] code", "```", "- [ ] real"];
		expect(filed(note, 2, false, TODAY, uid)).toBeUndefined();
		expect(filed(note, 5, false, TODAY, uid)).toBeUndefined();
		expect(filed(note, 7, false, TODAY, uid)).toBeDefined();
	});
});

describe("§6.4 the checkbox is the status, in a note", () => {
	const note = (...body: string[]): string[] => ["---", "restask-list: Home", "---", "# Home", ...body];

	it("stamps a checked line and moves it under the done heading", () => {
		const before = note(`- [x] a ${A}`, "- [ ] b", "", "## Done", `- [x] old ✅ 2026-09-01 ${B}`, "");
		expect(statusRepaired(before, 4, TODAY, "Done")).toEqual(
			note("- [ ] b", "", "## Done", `- [x] a ✅ ${TODAY} ${A}`, `- [x] old ✅ 2026-09-01 ${B}`, ""),
		);
	});

	it("keeps a completion date the line already has, and un-indents a subtask", () => {
		const before = note("- [ ] parent", `\t- [x] child ✅ 2026-09-30 ${A}`, "", "## Done", "");
		expect(statusRepaired(before, 5, TODAY, "Done")).toEqual(note("- [ ] parent", "", "## Done", `- [x] child ✅ 2026-09-30 ${A}`, ""));
	});

	it("creates the done heading when the note has none", () => {
		expect(statusRepaired(note(`- [x] a ${A}`, "- [ ] b", ""), 4, TODAY, "Fatto")).toEqual(
			note("- [ ] b", "", "### Fatto", `- [x] a ✅ ${TODAY} ${A}`, ""),
		);
	});

	it("stamps a checked line of the done region in place", () => {
		expect(statusRepaired(note("## Done", `- [X] a ${A}`), 5, TODAY, "Done")).toEqual(note("## Done", `- [x] a ✅ ${TODAY} ${A}`));
	});

	it("restores an unchecked line of the done region to the bottom of the active list", () => {
		const before = note("- [ ] b", "", "## Done", `- [ ] a ✅ 2026-09-30 ${A}`, "");
		expect(statusRepaired(before, 7, TODAY, "Done")).toEqual(note("- [ ] b", `- [ ] a ${A}`, "", "## Done", ""));
	});

	it("drops the date of an unchecked active line", () => {
		expect(statusRepaired(note(`- [ ] a ✅ 2026-09-30 ${A}`), 4, TODAY, "Done")).toEqual(note(`- [ ] a ${A}`));
	});

	it("leaves a consistent line, and anything that is not a task", () => {
		const doc = note("- [ ] a", "prose", "## Done", `- [x] b ✅ ${TODAY}`);
		for (const idx of [3, 4, 5, 6, 7]) expect(statusRepaired(doc, idx, TODAY, "Done")).toBeUndefined();
	});

	it("registers and completes a line typed already checked", () => {
		const before = note("- [x] did it", "", "## Done", "");
		expect(settled(before, 4, NOTE, TODAY, uid)).toEqual({
			lines: note("", "## Done", `- [x] did it ✅ ${TODAY} 🆔 ${U}`, ""),
			uid: U,
		});
	});

	it("reports the UID of a line that needs nothing, so its mirror line can be looked at", () => {
		expect(settled(note(`- [ ] a 🔺 ${A}`), 4, NOTE, TODAY, uid)).toEqual({ lines: undefined, uid: A.slice(3) });
	});
});

describe("§7.4 the section a task is typed in", () => {
	it("is its priority: the line is registered with it and stays where it is", () => {
		const before = view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, "- [ ] new", "", "## Done", "");
		expect(settled(before, 7, VIEW, TODAY, uid)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, `- [ ] new 🔺 🆔 ${U}`, "", "## Done", ""),
		);
	});

	it("yields to a priority written on the line, which is filed by it", () => {
		const before = view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, "- [ ] new 🔽", "", "## Done", "");
		expect(settled(before, 7, VIEW, TODAY, uid)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, "", "## 🔽 Low Priority", `- [ ] new 🔽 🆔 ${U}`, "", "## Done", ""),
		);
	});

	it("gives none under No Priority, under Done, or in a note with such a heading", () => {
		const plain = view("## No Priority", "- [ ] new", "", "## Done", "");
		expect(settled(plain, 6, VIEW, TODAY, uid)?.lines).toEqual(view("## No Priority", `- [ ] new 🆔 ${U}`, "", "## Done", ""));
		const done = view("## Done", "- [ ] new", "");
		expect(settled(done, 6, VIEW, TODAY, uid)?.lines).toEqual(view("## No Priority", `- [ ] new 🆔 ${U}`, "", "## Done", ""));
		const note = ["---", "restask-list: Home", "---", "## 🔺 Highest Priority", "- [ ] new"];
		expect(settled(note, 4, NOTE, TODAY, uid)?.lines).toEqual([...note.slice(0, 4), `- [ ] new 🆔 ${U}`]);
	});

	it("does not come back to a registered task whose priority was taken off", () => {
		const before = view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, `- [ ] plain now ➕ ${TODAY} ${B}`, "", "## Done", "");
		expect(settled(before, 7, VIEW, TODAY, uid)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, "", "## No Priority", `- [ ] plain now ➕ ${TODAY} ${B}`, "", "## Done", ""),
		);
	});
});

describe("§7.5 a line of the view names its calendar", () => {
	// The same lines as crates/restask/tests/sync_engine.rs
	// (`the_calendar_of_a_line_is_settled_on_the_device…`, `a_line_of_todo_md_lives_in_the_calendar_it_names`).
	it("keeps the calendar the last thing before the UID, as its slug", () => {
		expect(registeredLine("- [ ] for work 📁 Work", TODAY, uid)).toBe(`- [ ] for work 📁 work 🆔 ${U}`);
		expect(registeredLine("- [ ] for work 📁 work  ", TODAY, uid, "highest")).toBe(`- [ ] for work 🔺 📁 work 🆔 ${U}`);
		expect(registeredLine("- [ ] for work 🔽 📁 work", TODAY, uid, "highest")).toBe(`- [ ] for work 🔽 📁 work 🆔 ${U}`);
		// A token that is not the end of the line stays where it was typed.
		expect(registeredLine("- [ ] 📁 work for work", TODAY, uid)).toBe(`- [ ] 📁 work for work 🆔 ${U}`);
	});

	it("is registered in the section it was typed in, calendar kept", () => {
		const before = view("## 🔺 Highest Priority", "- [ ] for work 📁 work", "", "## Done", "");
		expect(settled(before, 6, VIEW, TODAY, uid)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] for work 🔺 📁 work 🆔 ${U}`, "", "## Done", ""),
		);
	});

	it("is completed and reopened with its calendar", () => {
		const before = view("## 🔺 Highest Priority", `- [x] Update restask README 🔺 📁 work ${A}`, "", "## Done", "");
		const done = view("## Done", `- [x] Update restask README 🔺 ✅ ${TODAY} 📁 work ${A}`, "");
		expect(settled(before, 6, VIEW, TODAY, uid)?.lines).toEqual(done);
		const reopened = view("## Done", `- [ ] Update restask README 🔺 ✅ ${TODAY} 📁 work ${A}`, "");
		expect(settled(reopened, 6, VIEW, TODAY, uid)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] Update restask README 🔺 📁 work ${A}`, "", "## Done", ""),
		);
	});

	it("is written in the canonical tail", () => {
		const draft = parseLine(`- [ ] a 📁 Work ${A} ➕ 2026-09-01 📅 2026-09-03`)?.draft;
		expect(draft && canonicalLine("", "-", { ...draft, priority: "medium" })).toBe(`- [ ] a 🔼 📅 2026-09-03 ➕ 2026-09-01 📁 work ${A}`);
	});
});

describe("§7.4 a line moved to another section", () => {
	const moved = (lines: string[], idx: number) => settled(lines, idx, VIEW, TODAY, uid, false, true);
	const M = "[[Home#TODO|Home]]";

	it("takes the section's priority and stays: a task of the view's own", () => {
		const up = view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, `- [ ] plain ➕ ${TODAY} ${B}`, "", "## Done", "");
		expect(moved(up, 7)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, `- [ ] plain 🔺 ➕ ${TODAY} ${B}`, "", "## Done", ""),
		);
		const down = view("## 🔽 Low Priority", `- [ ] urgent 🔺 📅 2026-10-05 ${B}`, "", "## Done", "");
		expect(moved(down, 6)?.lines).toEqual(view("## 🔽 Low Priority", `- [ ] urgent 🔽 📅 2026-10-05 ${B}`, "", "## Done", ""));
	});

	it("loses its priority under No Priority", () => {
		const before = view("## No Priority", `- [ ] urgent 🔺 ${B}`, "", "## Done", "");
		expect(moved(before, 6)?.lines).toEqual(view("## No Priority", `- [ ] urgent ${B}`, "", "## Done", ""));
	});

	it("is filed by its emoji when it was edited where it stood, or moved under Done", () => {
		const before = view("## 🔽 Low Priority", `- [ ] urgent 🔺 ${B}`, "", "## Done", "");
		expect(settled(before, 6, VIEW, TODAY, uid)?.lines).toEqual(view("## 🔺 Highest Priority", `- [ ] urgent 🔺 ${B}`, "", "## Done", ""));
		const done = view("## Done", `- [ ] urgent 🔺 ${B}`, "");
		expect(moved(done, 6)?.lines).toEqual(view("## 🔺 Highest Priority", `- [ ] urgent 🔺 ${B}`, "", "## Done", ""));
	});

	it("leaves a line alone that is in its own section, and registers a pasted new line as a typed one", () => {
		const home = view("## 🔺 Highest Priority", `- [ ] urgent 🔺 ${B}`, "- [ ] pasted text", "", "## Done", "");
		expect(moved(home, 6)?.lines).toBeUndefined();
		expect(moved(home, 7)?.lines?.[7]).toBe(`- [ ] pasted text 🔺 🆔 ${U}`);
	});

	it("rewrites a mirror line, drops the copy put back where it was cut, and says what to carry", () => {
		const was = `- [ ] theirs ⏫ 📅 2026-10-05 ${M} ${A}`;
		const now = `- [ ] theirs 🔽 📅 2026-10-05 ${M} ${A}`;
		const before = view("## ⏫ High Priority", was, "", "## 🔽 Low Priority", `- [ ] low 🔽 ${B}`, was, "", "## Done", "");
		expect(moved(before, 10)).toEqual({
			lines: view("## 🔽 Low Priority", `- [ ] low 🔽 ${B}`, now, "", "## Done", ""),
			uid: A.slice(3),
			mirror: true,
			moved: { was, now },
		});
		const above = view("## 🔺 Highest Priority", was, "", "## ⏫ High Priority", was, `- [ ] high ⏫ ${B}`, "", "## Done", "");
		expect(moved(above, 6)?.lines).toEqual(
			view("## 🔺 Highest Priority", `- [ ] theirs 🔺 📅 2026-10-05 ${M} ${A}`, "", "## ⏫ High Priority", `- [ ] high ⏫ ${B}`, "", "## Done", ""),
		);
	});

	it("takes the priority off a mirror line moved under No Priority, for its note to follow", () => {
		const was = `- [ ] theirs ⏫ ${M} ${A}`;
		const before = view("## No Priority", was, "", "## Done", "");
		expect(moved(before, 6)).toEqual({
			lines: view("## No Priority", `- [ ] theirs ${M} ${A}`, "", "## Done", ""),
			uid: A.slice(3),
			mirror: true,
			moved: { was, now: `- [ ] theirs ${M} ${A}` },
		});
	});
});

describe("§7 the checkbox in the TODO.md view", () => {
	it("stamps a completed task of the view's own and files it under Done", () => {
		const before = view("## No Priority", `- [x] mine ➕ 2026-10-01 ${A}`, "", "## Done", "");
		expect(settled(before, 6, VIEW, TODAY, uid)?.lines).toEqual(view("## Done", `- [x] mine ✅ ${TODAY} ➕ 2026-10-01 ${A}`, ""));
	});

	it("reopens a task of the view's own: the date goes, the line returns to its section", () => {
		const before = view("## Done", `- [ ] mine 🔽 ✅ 2026-10-01 ➕ 2026-10-01 ${A}`, "");
		expect(settled(before, 6, VIEW, TODAY, uid)?.lines).toEqual(
			view("## 🔽 Low Priority", `- [ ] mine 🔽 ➕ 2026-10-01 ${A}`, "", "## Done", ""),
		);
	});

	it("hands a checked mirror line to its source note instead of editing the view", () => {
		const before = view("## 🔺 Highest Priority", `- [x] theirs 🔺 [[Home#TODO|Home]] ${A}`, "", "## Done", "");
		expect(settled(before, 6, VIEW, TODAY, uid)).toEqual({ uid: A.slice(3), mirror: true, carry: A.slice(3) });
		expect(settled(before, 6, VIEW, TODAY, uid, true)?.lines).toEqual(
			view("## Done", `- [x] theirs 🔺 [[Home#TODO|Home]] ✅ ${TODAY} ${A}`, ""),
		);
	});

	it("finds the note a mirror line links to", () => {
		expect(mirrorSource(`- [x] theirs 🔺 [[Home Lab#TODO|Home Lab]] ${A}`)).toBe("Home Lab");
		expect(mirrorSource(`- [x] theirs 🔺 [[Home|Home]] ${A}`)).toBe("Home");
		expect(mirrorSource(`- [x] see [[Other]] 🔺 Home#TODO ${A}`)).toBeUndefined();
	});

	it("completes the task in its note by UID", () => {
		const note = ["# Home", `- [ ] theirs 🔺 ${A}`, "- [ ] b", "", "## Done", ""];
		expect(completedByUid(note, A.slice(3), TODAY, "Done")).toEqual(["# Home", "- [ ] b", "", "## Done", `- [x] theirs 🔺 ✅ ${TODAY} ${A}`, ""]);
		expect(completedByUid(["## Done", `- [x] theirs ✅ ${TODAY} ${A}`], A.slice(3), TODAY, "Done")).toBeUndefined();
		expect(completedByUid(note, B.slice(3), TODAY, "Done")).toBe("absent");
		expect(completedByUid(["```", `- [ ] in code ${A}`, "```"], A.slice(3), TODAY, "Done")).toBe("absent");
	});
});

describe("§7 mirror lines of note tasks", () => {
	const a = A.slice(3);
	const note = ["---", "restask-list: Home", "---", "- [ ] top 🔼 " + A, "# Home Lab", `- [ ] renew the   certificate ⏫ 📅 2026-10-05 17:00 🛫 2026-10-01 🔁 every week ➕ ${TODAY} ${B}`, "- [ ] plain 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpd", "## Done", "- [x] done 🔺 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpe"];

	it("renders the line the engine renders (§7)", () => {
		expect(mirrorLine(note, B.slice(3), "Home Lab", "Done")).toBe(
			`- [ ] renew the certificate ⏫ 🔁 every week 🛫 2026-10-01 📅 2026-10-05 17:00 [[Home Lab#Home Lab|Home Lab]] ${B}`,
		);
		expect(mirrorLine(note, a, "home", "Done")).toBe(`- [ ] top 🔼 [[home|home]] ${A}`);
		expect(mirrorLine(note, a, "a|b", "Done")).toBe(`- [ ] top 🔼 a|b ${A}`);
	});

	it("has none for a task the view does not show", () => {
		expect(mirrorLine(note, "restask-01jzq4tsvg2c9xkw7n5m8rhdpd", "Home", "Done")).toBeUndefined();
		expect(mirrorLine(note, "restask-01jzq4tsvg2c9xkw7n5m8rhdpe", "Home", "Done")).toBeUndefined();
		expect(mirrorLine(note, "restask-01jzq4tsvg2c9xkw7n5m8rhdpf", "Home", "Done")).toBeUndefined();
	});

	it("adds a missing mirror line in its section", () => {
		const line = `- [ ] top 🔼 [[home|home]] ${A}`;
		expect(mirrored(view("## Done", ""), a, line)).toEqual(view("## 🔼 Medium Priority", line, "", "## Done", ""));
		const has = view("## 🔼 Medium Priority", `- [ ] other 🔼 ${B}`, "", "## Done", "");
		expect(mirrored(has, a, line)).toEqual(view("## 🔼 Medium Priority", `- [ ] other 🔼 ${B}`, line, "", "## Done", ""));
	});

	it("rewrites a mirror line that is there only when told the view is sealed (§7.2)", () => {
		const stale = view("## 🔺 Highest Priority", `- [ ] old text 🔺 [[home|home]] ${A}`, `- [ ] other 🔺 ${B}`, "", "## Done", "");
		// Unsealed: to the daemon a changed mirror line is an edit of the task.
		expect(mirrored(stale, a, `- [ ] top 🔼 [[home|home]] ${A}`)).toBeUndefined();
		expect(mirrored(stale, a, `- [ ] new text 🔺 [[home|home]] ${A}`, true)).toEqual(
			view("## 🔺 Highest Priority", `- [ ] new text 🔺 [[home|home]] ${A}`, `- [ ] other 🔺 ${B}`, "", "## Done", ""),
		);
		expect(mirrored(stale, a, `- [ ] top 🔼 [[home|home]] ${A}`, true)).toEqual(
			view("## 🔺 Highest Priority", `- [ ] other 🔺 ${B}`, "", "## 🔼 Medium Priority", `- [ ] top 🔼 [[home|home]] ${A}`, "", "## Done", ""),
		);
		expect(mirrored(stale, a, `- [ ] old text 🔺 [[home|home]] ${A}`, true)).toBeUndefined();
	});

	it("removes the mirror line of a task the view no longer shows", () => {
		const has = view("## 🔺 Highest Priority", `- [ ] top 🔺 [[home|home]] ${A}`, "", "## Done", "");
		expect(mirrored(has, a, undefined)).toEqual(view("## Done", ""));
		expect(mirrored(view("## Done", ""), a, undefined)).toBeUndefined();
	});

	it("never removes a task of the view's own that happens to share the UID", () => {
		const own = view("## No Priority", `- [ ] mine ${A}`, "", "## Done", "");
		expect(mirrored(own, a, undefined)).toBeUndefined();
	});
});

describe("§7.1 a deleted mirror line deletes the task in its note", () => {
	const a = A.slice(3);
	const note = ["---", "restask-list: home", "---", "# Home", `- [ ] top 🔺 ${A}`, `- [ ] other 🔺 ${B}`, "prose", ""];
	const mirror = `- [ ] top 🔺 [[home#Home|home]] ${A}`;

	it("takes the task's line out of the note, and nothing else", () => {
		expect(mirrorDropped(note, a, mirror, "home", "Done")).toEqual({
			lines: ["---", "restask-list: home", "---", "# Home", `- [ ] other 🔺 ${B}`, "prose", ""],
			index: 4,
			line: `- [ ] top 🔺 ${A}`,
		});
	});

	it("lets the note win when it changed the task since: the view shows the line again", () => {
		const reworded = note.map((line) => line.replace("top", "top of the list"));
		expect(mirrorDropped(reworded, a, mirror, "home", "Done")).toEqual({ restore: `- [ ] top of the list 🔺 [[home#Home|home]] ${A}` });
	});

	it("does nothing when the note has no such task, or none the view would show", () => {
		expect(mirrorDropped(["# Home", ""], a, mirror, "home", "Done")).toBeUndefined();
		expect(mirrorDropped(note.map((line) => line.replace("top 🔺", "top")), a, mirror, "home", "Done")).toBeUndefined();
		expect(mirrorDropped(note.map((line) => line.replace("[ ] top", "[x] top")), a, mirror, "home", "Done")).toBeUndefined();
	});

	it("puts the line back where it was when the mirror line returns", () => {
		const dropped = mirrorDropped(note, a, mirror, "home", "Done");
		expect(dropped !== undefined && "lines" in dropped).toBe(true);
		if (dropped === undefined || !("lines" in dropped)) return;
		expect(putBack(dropped.lines, a, dropped.index, dropped.line)).toEqual(note);
		expect(putBack(note, a, dropped.index, dropped.line)).toBeUndefined();
		expect(putBack(["# Home"], a, 9, dropped.line)).toEqual(["# Home", dropped.line]);
	});

	it("makes a view the plugin took a line out of claim no render", () => {
		const view = ["---", "restask-list: inbox", "restask-render: 5d0d649d4cd83964", "---", "", "## Done", ""];
		expect(disclaimed(view)[2]).toBe("restask-render: 0000000000000000");
		expect(isSealed(disclaimed(view))).toBe(false);
		expect(resealed(disclaimed(view))).toEqual(resealed(view));
		expect(disclaimed(["# no frontmatter"])).toEqual(["# no frontmatter"]);
	});
});

describe("§7.2 the seal", () => {
	const empty = ["---", "restask-list: inbox", "---", "# TODO", "", "## Done", ""];
	const seal = "restask-render: c5cb3ffaf7a952cc";
	const sealedEmpty = [...empty.slice(0, 2), seal, ...empty.slice(2)];

	it("is the engine's: FNV-1a 64 of the view without its seal line", () => {
		// The same view and digest are pinned in crates/restask/tests/todo_view.rs.
		expect(digest(empty.join("\n"))).toBe("c5cb3ffaf7a952cc");
		expect(digest("")).toBe("cbf29ce484222325");
		expect(digest("🔺")).toBe(digest("\u{1F53A}"));
		expect(isSealed(sealedEmpty)).toBe(true);
	});

	it("is a property of the frontmatter: the view's body holds no line of restask's", () => {
		expect(sealedEmpty.some((line) => line.includes("<!--"))).toBe(false);
		// The same line below the frontmatter, or in a file without one, seals nothing.
		expect(isSealed([...empty.slice(0, 3), seal, ...empty.slice(3)])).toBe(false);
		expect(isSealed([seal, ...empty.slice(3)])).toBe(false);
		expect(isSealed(["---", "restask-list: inbox", seal])).toBe(false);
		// Nor do the comment lines of an earlier render.
		const comment = "<!-- restask-render: 5d0d649d4cd83964 -->";
		const legacy = [...empty.slice(0, 3), comment, ...empty.slice(3)];
		expect(isSealed(legacy)).toBe(false);
		expect(resealed(legacy)).toEqual(legacy);
	});

	it("breaks with any edit and is absent from an unsealed view", () => {
		expect(isSealed([...sealedEmpty.slice(0, 6), "- [ ] typed", ...sealedEmpty.slice(6)])).toBe(false);
		expect(isSealed(empty)).toBe(false);
		expect(isSealed([seal])).toBe(false);
		expect(isSealed([])).toBe(false);
	});

	it("is renewed after the plugin's own edit, and never added to a view that has none", () => {
		const edited = [...sealedEmpty.slice(0, 6), "## No Priority", "- [ ] typed", "", ...sealedEmpty.slice(6)];
		const again = resealed(edited);
		expect(isSealed(again)).toBe(true);
		expect(again.filter((_l, i) => i !== 2)).toEqual(edited.filter((_l, i) => i !== 2));
		expect(resealed(empty)).toEqual(empty);
	});
});

describe("§7.1 an edit made on a mirror line reaches the note", () => {
	const a = A.slice(3);
	const note = ["# Home", `  * [ ] call the bank 🔺 📅 2026-10-05 ➕ 2026-10-01 ${A}`, "- [ ] other"];
	const mirror = (rest: string): string => `- [ ] ${rest} [[Home#Home|Home]] ${A}`;
	const edit = (was: string, now: string, lines = note): unknown => mirrorEdited(lines, a, mirror(was), mirror(now), "Home", "Done");

	it("takes over the fields the user changed, in canonical form", () => {
		expect(edit("call the bank 🔺 📅 2026-10-05", "call the bank today 🔽 📅 2026-10-06 🔁 every week")).toEqual([
			"# Home",
			`  * [ ] call the bank today 🔽 🔁 every week 📅 2026-10-06 ➕ 2026-10-01 ${A}`,
			"- [ ] other",
		]);
		expect(edit("call the bank 🔺 📅 2026-10-05", "call the bank 📅 2026-10-05")).toEqual([
			"# Home",
			`  * [ ] call the bank 📅 2026-10-05 ➕ 2026-10-01 ${A}`,
			"- [ ] other",
		]);
	});

	it("lets the note win where it changed the field too", () => {
		// The mirror line still showed 🔼 when the user made it 🔽; the note says 🔺 by now.
		expect(edit("call the bank 🔼 📅 2026-10-05", "call the bank 🔽 📅 2026-10-05")).toBeUndefined();
		// Field by field: the date the note still shows as it was rendered is taken over.
		expect(edit("call the bank 🔼 📅 2026-10-05", "call the bank 🔽 📅 2026-10-09")).toEqual([
			"# Home",
			`  * [ ] call the bank 🔺 📅 2026-10-09 ➕ 2026-10-01 ${A}`,
			"- [ ] other",
		]);
		expect(edit("call 🔺 📅 2026-10-05", "call them 🔺 📅 2026-10-05")).toBeUndefined();
	});

	it("does not touch the text when the link was edited, and nothing when nothing changed", () => {
		const broken = mirrorEdited(note, a, mirror("call the bank 🔺 📅 2026-10-05"), `- [ ] call 🔺 📅 2026-10-05 [[Elsewhere]] ${A}`, "Home", "Done");
		expect(broken).toBeUndefined();
		expect(edit("call the bank 🔺 📅 2026-10-05", "call the bank 🔺 📅 2026-10-05")).toBeUndefined();
	});

	it("says when the note has no such task", () => {
		expect(edit("a 🔺", "b 🔺", ["# Home", "- [ ] other"])).toBe("absent");
	});

	it("writes a line the way the engine does", () => {
		const draft = parseLine(`- [x] t ${A} ➕ 2026-10-01 ✅ 2026-10-02 📅 2026-10-05 10:00 ⏳ 2026-10-04 🛫 2026-10-03 🔁 every day ⏫`)?.draft;
		expect(draft).toBeDefined();
		if (draft === undefined) return;
		expect(canonicalLine("\t", "+", draft)).toBe(
			`\t+ [x] t ⏫ 🔁 every day 🛫 2026-10-03 ⏳ 2026-10-04 📅 2026-10-05 10:00 ✅ 2026-10-02 ➕ 2026-10-01 ${A}`,
		);
	});
});

describe("§6.4 a copied line gets its own UID", () => {
	const fresh = (): string => U;

	it("the first occurrence keeps the UID, whichever of the two was edited", () => {
		const note = ["---", "restask-list: Home", "---", `- [ ] a ${A}`, `- [ ] a copy ${A}`];
		const expected = [...note.slice(0, 4), `- [ ] a copy 🆔 ${U}`];
		expect(settled(note, 4, NOTE, TODAY, fresh)).toEqual({ lines: expected, uid: U });
		expect(settled(note, 3, NOTE, TODAY, fresh)).toEqual({ lines: expected, uid: A.slice(3) });
	});

	it("a mirror line and its own task in the view are not copies of each other", () => {
		const doc = view("## 🔺 Highest Priority", `- [ ] a 🔺 [[Home|Home]] ${A}`, `- [ ] a 🔺 [[Home|Home]] ${A}`, "", "## Done", "");
		expect(settled(doc, 7, VIEW, TODAY, fresh)?.lines).toBeUndefined();
		const own = view("## No Priority", `- [ ] mine ${A}`, `- [ ] mine too ${A}`, "", "## Done", "");
		expect(settled(own, 7, VIEW, TODAY, fresh)?.lines).toEqual(view("## No Priority", `- [ ] mine ${A}`, `- [ ] mine too 🆔 ${U}`, "", "## Done", ""));
	});
});

describe("§15.6 a checkbox tapped in reading view", () => {
	it("is the one line whose box differs", () => {
		expect(toggledLines(["# t", "- [ ] a", "\t* [x] b"], ["# t", "- [x] a", "\t* [x] b"])).toEqual([1]);
		expect(toggledLines(["- [x] a"], ["- [ ] a"])).toEqual([0]);
	});

	it("is nothing when the note changed in any other way", () => {
		expect(toggledLines(["- [ ] a"], ["- [x] a!"])).toEqual([]);
		expect(toggledLines(["- [ ] a"], ["- [x] a", ""])).toEqual([]);
		expect(toggledLines(["- [ ] a", "x"], ["- [x] a", "y"])).toEqual([]);
		expect(toggledLines(["a"], ["a"])).toEqual([]);
	});
});

describe("§15.6 the change as splices", () => {
	it("is empty for equal documents", () => {
		expect(splices(["a", "b"], ["a", "b"])).toEqual([]);
	});

	it("is one line for a registration", () => {
		expect(splices(["a", "b", "c"], ["a", "B", "c"])).toEqual([{ at: 1, remove: 1, insert: ["B"] }]);
	});

	it("leaves the lines a moved line passes untouched", () => {
		const up = splices(["h1", "x", "", "h2", "moved", "tail"], ["h1", "x", "moved!", "", "h2", "tail"]);
		expect(up).toEqual([
			{ at: 2, remove: 0, insert: ["moved!"] },
			{ at: 4, remove: 1, insert: [] },
		]);
		const down = splices(["h1", "moved", "cursor", "h2", "x", ""], ["h1", "cursor", "h2", "x", "moved!", ""]);
		expect(down).toEqual([
			{ at: 1, remove: 1, insert: [] },
			{ at: 5, remove: 0, insert: ["moved!"] },
		]);
	});

	it("reproduces every refiling when applied", () => {
		const cases: [string[], number][] = [
			[view("## 🔺 Highest Priority", `- [ ] old 🔺 ${A}`, "", "## Done", "- [ ] new 🔺", ""), 9],
			[view("## No Priority", "- [ ] now urgent 🔺", "", "## Done", ""), 6],
			[view("## No Priority", "- [ ] going down 🔽", "- [ ] stays", "", "## Done", ""), 6],
			[view("## Done", "- [ ] new", ""), 6],
			[[...HEAD, "- [ ] new 🔽"], 5],
			[["- [ ] only"], 0],
		];
		for (const [before, idx] of cases) {
			const after = filed(before, idx, true, TODAY, uid);
			expect(after).toBeDefined();
			expect(patch(before, after ?? [])).toEqual(after);
			expect(splices(before, after ?? []).length).toBeLessThanOrEqual(2);
		}
	});
});
