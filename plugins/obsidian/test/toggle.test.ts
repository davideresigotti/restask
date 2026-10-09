//! §15.3 toggle tests: completing and reopening as pure document transformations, in
//! notes and in the TODO.md view.

import { describe, expect, it } from "vitest";
import { toggleDone } from "../src/toggle";

const TODAY = "2026-09-23";
const A = "🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb";
const B = "🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpc";

function toggle(doc: string, line: number) {
	const result = toggleDone(doc, line, TODAY, "Done");
	if (result === undefined) throw new Error("not a task line");
	return result;
}

describe("§15.3 completing", () => {
	it("stamps, flips and moves the line under the done heading, newest on top", () => {
		const doc = [`- [ ] A ➕ 2026-09-01 ${A}`, "- [ ] B", "", "## Done", `- [x] old ✅ 2026-09-01 ${B}`, ""].join("\n");
		const out = toggle(doc, 0);
		expect(out.completed).toBe(true);
		expect(out.doc).toBe(
			["- [ ] B", "", "## Done", `- [x] A ✅ ${TODAY} ➕ 2026-09-01 ${A}`, `- [x] old ✅ 2026-09-01 ${B}`, ""].join("\n"),
		);
		expect(out.line).toBe(3);
	});

	it("puts the stamp in front of a counted UID (§3.1) as in front of a long one", () => {
		const out = toggle("- [ ] A ➕ 2026-09-01 🆔 a42\n- [ ] B 🆔 a4\n\n## Done\n", 1);
		expect(out.doc).toBe(`- [ ] A ➕ 2026-09-01 🆔 a42\n\n## Done\n- [x] B ✅ ${TODAY} 🆔 a4\n`);
	});

	it("creates a level-3 heading at the end when there is none", () => {
		const out = toggle("# Notes\n\n- [ ] only task\n", 2);
		expect(out.doc).toBe(`# Notes\n\n### Done\n- [x] only task ✅ ${TODAY}\n`);
		expect(out.line).toBe(3);

		const unterminated = toggle("- [ ] a\n- [ ] b", 0);
		expect(unterminated.doc).toBe(`- [ ] b\n\n### Done\n- [x] a ✅ ${TODAY}\n`);
	});

	it("uses the configured done heading", () => {
		const out = toggleDone("- [ ] a\n\n## Fatto\n", 0, TODAY, "Fatto");
		expect(out?.doc).toBe(`\n## Fatto\n- [x] a ✅ ${TODAY}\n`);
	});

	it("puts the stamp ahead of a calendar token in the tail, and nowhere else (§7.5)", () => {
		const view = (line: string): string => ["## No Priority", line, "", "## Done", ""].join("\n");
		expect(toggle(view(`- [ ] A 📁 work ${A}`), 1).doc).toBe(["## No Priority", "", "## Done", `- [x] A ✅ ${TODAY} 📁 work ${A}`, ""].join("\n"));
		expect(toggle(view("- [ ] A 📁 work"), 1).doc).toBe(["## No Priority", "", "## Done", `- [x] A ✅ ${TODAY} 📁 work`, ""].join("\n"));
		// A token in the middle of the text is not the tail.
		expect(toggle(view(`- [ ] 📁 work A ${A}`), 1).doc).toBe(["## No Priority", "", "## Done", `- [x] 📁 work A ✅ ${TODAY} ${A}`, ""].join("\n"));
	});

	it("a subtask leaves its indentation behind", () => {
		const doc = [`- [ ] parent ${A}`, `    - [ ] child ${B}`, "## Done"].join("\n");
		const out = toggle(doc, 1);
		expect(out.doc).toBe([`- [ ] parent ${A}`, "## Done", `- [x] child ✅ ${TODAY} ${B}`].join("\n"));
	});

	it("preserves every other byte of the line and the document", () => {
		const doc = ["intro  text", "* [ ] keep   my  spacing ⏫ 📅 2026-10-01", "## Done", "tail"].join("\n");
		const out = toggle(doc, 1);
		expect(out.doc).toBe(
			["intro  text", "## Done", `* [x] keep   my  spacing ⏫ 📅 2026-10-01 ✅ ${TODAY}`, "tail"].join("\n"),
		);
	});

	it("an unchecked line already under the done heading is completed in place", () => {
		const doc = ["## Done", "- [ ] forgotten box"].join("\n");
		const out = toggle(doc, 1);
		expect(out.doc).toBe(["## Done", `- [x] forgotten box ✅ ${TODAY}`].join("\n"));
		expect(out.line).toBe(1);
	});

	it("works on a TODO.md mirror line (the daemon carries it to the note)", () => {
		const doc = [
			"## ⏫ High Priority",
			`- [ ] Review ⏫ [[Alpha#Tasks|Alpha]] ${A}`,
			"",
			"## Done",
			"",
		].join("\n");
		const out = toggle(doc, 1);
		expect(out.doc).toBe(
			["## ⏫ High Priority", "", "## Done", `- [x] Review ⏫ [[Alpha#Tasks|Alpha]] ✅ ${TODAY} ${A}`, ""].join("\n"),
		);
	});
});

describe("§15.3 reopening", () => {
	it("removes the stamp and returns the line to the bottom of the active list", () => {
		const doc = ["# T", `- [ ] A ${A}`, "", "## Done", `- [x] B ✅ 2026-09-20 ➕ 2026-09-01 ${B}`].join("\n");
		const out = toggle(doc, 4);
		expect(out.completed).toBe(false);
		expect(out.doc).toBe(["# T", `- [ ] A ${A}`, `- [ ] B ➕ 2026-09-01 ${B}`, "", "## Done"].join("\n"));
		expect(out.line).toBe(2);
	});

	it("goes above the done heading when no active task is left", () => {
		const doc = ["# T", "", "## Done", `- [x] B ✅ 2026-09-20 ${B}`].join("\n");
		const out = toggle(doc, 3);
		expect(out.doc).toBe(["# T", `- [ ] B ${B}`, "", "## Done"].join("\n"));
	});

	it("a checked line outside the done region is reopened in place", () => {
		const out = toggle(`- [x] stray ✅ 2026-09-20 ${A}\n- [ ] other`, 0);
		expect(out.doc).toBe(`- [ ] stray ${A}\n- [ ] other`);
		expect(out.line).toBe(0);
	});

	it("complete then reopen restores the task content", () => {
		const doc = `- [ ] A ➕ 2026-09-01 ${A}\n- [ ] B\n`;
		const done = toggle(doc, 0);
		const back = toggle(done.doc, done.line);
		expect(back.doc).toContain(`- [ ] A ➕ 2026-09-01 ${A}`);
		expect(back.doc).not.toContain("✅");
	});
});

describe("§15.3 non-tasks", () => {
	it("returns undefined off a task line", () => {
		expect(toggleDone("# heading\nplain", 0, TODAY, "Done")).toBeUndefined();
		expect(toggleDone("# heading\nplain", 1, TODAY, "Done")).toBeUndefined();
		expect(toggleDone("- [ ] a", 5, TODAY, "Done")).toBeUndefined();
	});
});
