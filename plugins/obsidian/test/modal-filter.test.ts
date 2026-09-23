//! §15.2 autocomplete filter tests: ≥ 2-letter threshold, `hi` → high + highest only,
//! case-insensitivity, keyword table inserts, today/tomorrow date arithmetic.

import { describe, expect, it } from "vitest";
import { addDays, MIN_FRAGMENT_LENGTH, suggestionsFor } from "../src/modal";

const TODAY = "2026-09-23";

describe("§15.2 modal filter", () => {
	it("threshold is two letters", () => {
		expect(MIN_FRAGMENT_LENGTH).toBe(2);
	});

	it("no suggestions below the two-letter threshold", () => {
		expect(suggestionsFor("", TODAY)).toEqual([]);
		expect(suggestionsFor("h", TODAY)).toEqual([]);
		expect(suggestionsFor("d", TODAY)).toEqual([]);
	});

	it("hi suggests high and highest only", () => {
		const keywords = suggestionsFor("hi", TODAY).map((s) => s.keyword);
		expect(keywords).toEqual(["highest", "high"]);
		expect(keywords).not.toContain("low");
		expect(keywords).not.toContain("lowest");
	});

	it("filter is case-insensitive", () => {
		expect(suggestionsFor("HI", TODAY)).toEqual(suggestionsFor("hi", TODAY));
		expect(suggestionsFor("ToDay", TODAY).map((s) => s.keyword)).toEqual(["today"]);
	});

	it("priority keywords insert their emoji", () => {
		const insertFor = (kw: string): string | undefined =>
			suggestionsFor(kw, TODAY).find((s) => s.keyword === kw)?.insert;
		expect(insertFor("highest")).toBe("🔺");
		expect(insertFor("high")).toBe("⏫");
		expect(insertFor("medium")).toBe("🔼");
		expect(insertFor("low")).toBe("🔽");
		expect(insertFor("lowest")).toBe("⏬");
	});

	it("date fields insert their token with trailing space", () => {
		expect(suggestionsFor("due", TODAY)).toEqual([{ keyword: "due", insert: "📅 " }]);
		expect(suggestionsFor("start", TODAY)).toEqual([{ keyword: "start", insert: "🛫 " }]);
		expect(suggestionsFor("scheduled", TODAY)).toEqual([{ keyword: "scheduled", insert: "⏳ " }]);
	});

	it("today and tomorrow insert dated due tokens", () => {
		expect(suggestionsFor("today", TODAY)).toEqual([{ keyword: "today", insert: `📅 ${TODAY}` }]);
		expect(suggestionsFor("tomorrow", TODAY)).toEqual([
			{ keyword: "tomorrow", insert: "📅 2026-09-24" },
		]);
	});

	it("two-letter fragment matches both today and tomorrow", () => {
		expect(suggestionsFor("to", TODAY).map((s) => s.keyword)).toEqual(["today", "tomorrow"]);
	});

	it("zero matches hide the menu", () => {
		expect(suggestionsFor("zz", TODAY)).toEqual([]);
	});

	it("addDays rolls over months and leap years", () => {
		expect(addDays("2026-09-30", 1)).toBe("2026-10-01");
		expect(addDays("2028-02-28", 1)).toBe("2028-02-29");
		expect(addDays("2026-02-28", 1)).toBe("2026-03-01");
		expect(addDays("2026-12-31", 1)).toBe("2027-01-01");
	});
});
