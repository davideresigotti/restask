//! §6.1–§6.2 grammar-port conformance tests: same case matrix as the Rust
//! `tests/markdown_parser.rs` suite, run against byte-exact copies of `test-vault/`.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
	linkParents,
	parse,
	parseLine,
	PRIORITY_EMOJI,
	PRIORITIES,
	type TaskDraft,
	type TaskLine,
} from "../src/markdown";

const UID = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpf";

function readFixture(name: string): string {
	return readFileSync(fileURLToPath(new URL(`fixtures/${name}`, import.meta.url)), "utf-8");
}

const HOME_LAB = readFixture("Home Lab Test.md");
const PROJECT_ALPHA = readFixture("Project Alpha Test.md");
const TODO = readFixture("TODO.md");

function draft(text: string): TaskDraft {
	return {
		uid: undefined,
		text,
		checked: false,
		priority: undefined,
		due: undefined,
		start: undefined,
		scheduled: undefined,
		created: undefined,
		completedOn: undefined,
	};
}

function mustParse(line: string): TaskLine {
	const t = parseLine(line);
	if (t === undefined) throw new Error(`line should match the §6.1 grammar: ${line}`);
	return t;
}

describe("§6.1 line grammar", () => {
	it("basic task line", () => {
		const t = mustParse("- [ ] Buy milk");
		expect(t.indentChars).toBe(0);
		expect(t.marker).toBe("-");
		expect(t.draft).toEqual(draft("Buy milk"));
		expect(t.draft.checked).toBe(false);
	});

	it("indent counting", () => {
		expect(mustParse("  - [ ] a").indentChars).toBe(2);
		expect(mustParse("\t- [ ] a").indentChars).toBe(1);
		expect(mustParse(" \t  - [ ] a").indentChars).toBe(4);
	});

	it("checkbox case", () => {
		expect(mustParse("- [ ] a").draft.checked).toBe(false);
		expect(mustParse("- [x] a").draft.checked).toBe(true);
		expect(mustParse("- [X] a").draft.checked).toBe(true);
	});

	it("list markers", () => {
		expect(mustParse("- [ ] a").marker).toBe("-");
		expect(mustParse("* [ ] a").marker).toBe("*");
		expect(mustParse("+ [ ] a").marker).toBe("+");
	});

	it("crlf and bare cr endings", () => {
		const expected = mustParse("- [ ] Buy milk");
		expect(mustParse("- [ ] Buy milk\r\n")).toEqual(expected);
		expect(mustParse("- [ ] Buy milk\r")).toEqual(expected);
	});

	it("non task lines", () => {
		for (const line of [
			"1. [ ] ordered list",
			"-[ ] no space after marker",
			"- [x]no space after bracket",
			"- [ ]no space after bracket",
			"- [y] unknown check",
			"- [ ]",
			"plain text line",
			"## Heading",
			"",
			"  ",
		]) {
			expect(parseLine(line)).toBeUndefined();
		}
	});

	it("unknown emoji stay in text", () => {
		const t = mustParse("- [ ] Buy milk ❗ 🔥 ➜");
		expect(t.draft.priority).toBeUndefined();
		expect(t.draft).toEqual(draft("Buy milk ❗ 🔥 ➜"));
	});

	it("priority must be standalone", () => {
		const t = mustParse("- [ ] Buy🔺milk");
		expect(t.draft.priority).toBeUndefined();
		expect(t.draft.text).toBe("Buy🔺milk");
	});

	it("all five priorities recognized", () => {
		for (const p of PRIORITIES) {
			expect(mustParse(`- [ ] x ${PRIORITY_EMOJI[p]}`).draft.priority).toBe(p);
		}
	});

	it("priority token at body start", () => {
		const t = mustParse("- [ ] 🔺 Buy milk");
		expect(t.draft.priority).toBe("highest");
		expect(t.draft.text).toBe("Buy milk");
	});

	it("all tokens any order", () => {
		const t = mustParse(
			"- [ ] Pay rent ⏳ 2026-01-02 🛫 2026-01-01 📅 2026-01-03 17:30 ➕ 2025-12-31 ✅ 2026-01-04 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpf 🔼",
		);
		expect(t.draft.text).toBe("Pay rent");
		expect(t.draft.priority).toBe("medium");
		expect(t.draft.scheduled).toEqual({ kind: "date", date: "2026-01-02" });
		expect(t.draft.start).toEqual({ kind: "date", date: "2026-01-01" });
		expect(t.draft.due).toEqual({ kind: "datetime", date: "2026-01-03", time: "17:30" });
		expect(t.draft.created).toBe("2025-12-31");
		expect(t.draft.completedOn).toBe("2026-01-04");
		expect(t.draft.uid).toBe(UID);
		expect(t.draft.checked).toBe(false);
	});

	it("due date and datetime forms", () => {
		expect(mustParse("- [ ] a 📅 2026-01-03").draft.due).toEqual({ kind: "date", date: "2026-01-03" });
		expect(mustParse("- [ ] a 📅 2026-01-03 17:30").draft.due).toEqual({
			kind: "datetime",
			date: "2026-01-03",
			time: "17:30",
		});
		expect(mustParse("- [ ] a 📅\t2026-01-03").draft.due).toEqual({ kind: "date", date: "2026-01-03" });
	});

	it("token glued to text is still a token", () => {
		const t = mustParse("- [ ] Buy milk📅 2026-01-01");
		expect(t.draft.due).toEqual({ kind: "date", date: "2026-01-01" });
		expect(t.draft.text).toBe("Buy milk");
	});

	it("invalid token values tolerated", () => {
		const t = mustParse("- [ ] a 📅 2026-13-45 ➕ 2026-02-30 ✅ 2026-00-11");
		expect(t.draft.due).toBeUndefined();
		expect(t.draft.created).toBeUndefined();
		expect(t.draft.completedOn).toBeUndefined();
		expect(t.draft.text).toBe("a");
	});

	it("uid with invalid ulid body tolerated", () => {
		const t = mustParse(`- [ ] a 🆔 taskres-iou${"a".repeat(23)}`);
		expect(t.draft.uid).toBeUndefined();
		expect(t.draft.text).toBe("a");
	});

	it("duplicate tokens first wins all removed", () => {
		const t = mustParse("- [ ] a 📅 2026-01-03 📅 2026-02-04");
		expect(t.draft.due).toEqual({ kind: "date", date: "2026-01-03" });
		expect(t.draft.text).toBe("a");
	});

	it("text whitespace normalized", () => {
		expect(mustParse("- [ ] \tBuy   milk\t \t📅 2026-01-01").draft.text).toBe("Buy milk");
	});

	it("empty body tasks", () => {
		expect(mustParse("- [ ] ").draft.text).toBe("");
		const t = mustParse("- [ ]  📅 2026-01-01");
		expect(t.draft.text).toBe("");
		expect(t.draft.due).toEqual({ kind: "date", date: "2026-01-01" });
	});
});

describe("§6.2 file-level rules", () => {
	it("home lab note done split and headings", () => {
		const f = parse(HOME_LAB);
		expect(f.doneHeadingLine).toBe(10);
		expect(f.tasks).toHaveLength(5);

		const active = f.tasks.filter((t) => !t.inDoneRegion);
		expect(active).toHaveLength(4);
		expect(active[0].lineNo).toBe(5);
		expect(active[0].draft.text).toBe("Clean up cable management");
		expect(active[0].heading).toBe("TODO");
		expect(active[1].draft.priority).toBe("medium");
		expect(active[2].draft.priority).toBe("low");
		expect(active[3].draft.text).toBe("Test the plugin high");
		expect(active[3].draft.priority).toBeUndefined();

		const done = f.tasks.filter((t) => t.inDoneRegion);
		expect(done).toHaveLength(1);
		expect(done[0].lineNo).toBe(11);
		expect(done[0].draft.checked).toBe(true);
		expect(done[0].draft.priority).toBe("highest");
		expect(done[0].draft.completedOn).toBe("2026-09-19");
		expect(done[0].heading).toBe("Done");
		expect(linkParents(f.tasks).every((p) => p === undefined)).toBe(true);
	});

	it("project alpha done region extends to eof", () => {
		const f = parse(PROJECT_ALPHA);
		expect(f.doneHeadingLine).toBe(6);
		expect(f.tasks).toHaveLength(2);
		expect(f.tasks.every((t) => t.inDoneRegion)).toBe(true);
		expect(f.tasks[0].draft.priority).toBe("high");
		expect(f.tasks[0].draft.text).toBe("Review architecture plan");
		expect(f.tasks[1].draft.priority).toBe("medium");
		expect(f.tasks[1].heading).toBe("Done");
	});

	it("todo md view file parses", () => {
		const f = parse(TODO);
		expect(f.doneHeadingLine).toBe(20);
		expect(f.tasks).toHaveLength(7);

		const first = f.tasks[0];
		expect(first.lineNo).toBe(6);
		expect(first.heading).toBe("🔺 Highest Priority");
		expect(first.draft.priority).toBe("highest");
		expect(first.draft.text).toBe(
			"Setup SSL certificate renew alert [[Home Lab Test#To Do|Home Lab Test]]",
		);

		const done = f.tasks.filter((t) => t.inDoneRegion);
		expect(done).toHaveLength(1);
		expect(done[0].lineNo).toBe(21);
		expect(done[0].draft.text).toBe("Take out trash");
	});

	it("frontmatter lines are never tasks", () => {
		const f = parse("---\nrestask-list: Home\n- [ ] ghost in frontmatter\n---\n- [ ] real task\n");
		expect(f.tasks).toHaveLength(1);
		expect(f.tasks[0].lineNo).toBe(5);
		expect(f.tasks[0].draft.text).toBe("real task");
	});

	it("unterminated frontmatter is not frontmatter", () => {
		const f = parse("---\n- [ ] ghost\n");
		expect(f.tasks).toHaveLength(1);
		expect(f.tasks[0].lineNo).toBe(2);
		expect(f.tasks[0].draft.text).toBe("ghost");
	});

	it("fenced blocks are never tasks", () => {
		const f = parse(
			"```tasks\n" +
				"- [ ] in tasks query\n" +
				"```\n" +
				"- [ ] real one\n" +
				"~~~\n" +
				"- [ ] in tilde fence\n" +
				"~~~\n" +
				"- [ ] another real\n",
		);
		expect(f.tasks).toHaveLength(2);
		expect(f.tasks[0].draft.text).toBe("real one");
		expect(f.tasks[1].draft.text).toBe("another real");
	});

	it("crlf file lines", () => {
		const f = parse("# T\r\n\r\n- [ ] a 📅 2026-01-01\r\n");
		expect(f.tasks).toHaveLength(1);
		expect(f.tasks[0].raw).toBe("- [ ] a 📅 2026-01-01");
		expect(f.tasks[0].lineNo).toBe(3);
	});

	it("done heading config case and persistence", () => {
		const cfg = { doneHeading: "Completed" };
		const f = parse(
			"## done\n" +
				"- [ ] a\n" +
				"## Done ###\n" +
				"- [ ] b\n" +
				"## Completed\n" +
				"- [x] c\n" +
				"## Later\n" +
				"- [x] d\n",
			cfg,
		);
		expect(f.doneHeadingLine).toBe(5);
		expect(f.tasks[0].inDoneRegion).toBe(false);
		expect(f.tasks[1].inDoneRegion).toBe(false);
		expect(f.tasks[2].inDoneRegion).toBe(true);
		expect(f.tasks[3].inDoneRegion).toBe(true);
		expect(f.tasks[1].heading).toBe("Done");
		expect(f.tasks[3].heading).toBe("Later");
	});

	it("link parents nearest ancestor", () => {
		const u1 = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpb";
		const f = parse(
			"- [ ] A 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpb\n" +
				"    - [ ] B 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpc\n" +
				"    - [ ] C\n" +
				"        - [ ] D 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpd\n" +
				"- [ ] E\n" +
				"    - [ ] F 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpe\n",
		);
		expect(f.tasks[1].indentChars).toBe(4);
		const parents = linkParents(f.tasks);
		expect(parents[0]).toBeUndefined();
		expect(parents[1]).toBe(u1);
		expect(parents[2]).toBe(u1);
		expect(parents[3]).toBeUndefined();
		expect(parents[4]).toBeUndefined();
		expect(parents[5]).toBeUndefined();
	});
});
