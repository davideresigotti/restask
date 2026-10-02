//! §15.5 locating the hidden `🆔` token: which lines have one, and what exactly is hidden.

import { describe, expect, it } from "vitest";
import { stripUid, uidToken } from "../src/conceal";

const UID = "restask-01jzq4tsvg2c9xkw7n5m8rhdpf";
const LEGACY = "taskres-01jzq4tsvg2c9xkw7n5m8rhdpf";

/** The line as the editor shows it: the hidden range cut out. */
function shown(line: string): string {
	const token = uidToken(line);
	return token === undefined ? line : line.slice(0, token.start) + line.slice(token.end);
}

describe("§15.5 uidToken", () => {
	it("hides the token and the blank before it", () => {
		expect(shown(`- [ ] Buy milk ➕ 2026-09-22 🆔 ${UID}`)).toBe("- [ ] Buy milk ➕ 2026-09-22");
		expect(shown(`\t* [x] Done\t🆔  ${UID}`)).toBe("\t* [x] Done");
		expect(shown(`- [ ] old 🆔 ${LEGACY}`)).toBe("- [ ] old");
	});

	it("hides one blank only: further ones are text the user typed, and can see", () => {
		expect(shown(`- [ ] Buy milk   🆔 ${UID}`)).toBe("- [ ] Buy milk  ");
		expect(shown(`\t* [x] Done \t🆔  ${UID}`)).toBe("\t* [x] Done ");
	});

	it("keeps what follows the token", () => {
		expect(shown(`- [ ] Buy 🆔 ${UID} milk`)).toBe("- [ ] Buy milk");
	});

	it("never reaches into the checkbox", () => {
		const line = `- [ ]   🆔 ${UID}`;
		expect(uidToken(line)).toEqual({ bodyStart: 8, start: 8, end: line.length });
		expect(shown(line)).toBe("- [ ]   ");
	});

	it("hides a mirror line's token after the wikilink", () => {
		expect(shown(`- [ ] Renew 🔺 [[Home Lab#Tasks|Home Lab]] 🆔 ${UID}`)).toBe(
			"- [ ] Renew 🔺 [[Home Lab#Tasks|Home Lab]]",
		);
	});

	it("leaves everything that is not a task's identity visible", () => {
		for (const line of [
			`Just prose 🆔 ${UID}`,
			`1. [ ] ordered 🆔 ${UID}`,
			"- [ ] no token",
			"- [ ] malformed 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpi",
			"- [ ] short 🆔 restask-01jzq4",
		]) {
			expect(uidToken(line)).toBeUndefined();
		}
	});

	it("hides only the first token, the one the parser reads", () => {
		const second = "restask-01jzq4tsvg2c9xkw7n5m8rhdpg";
		expect(shown(`- [ ] a 🆔 ${UID} 🆔 ${second}`)).toBe(`- [ ] a 🆔 ${second}`);
	});
});

describe("§15.5 stripUid (reading view)", () => {
	it("removes the token from rendered text", () => {
		expect(stripUid(` Buy milk ➕ 2026-09-22 🆔 ${UID}`)).toBe(" Buy milk ➕ 2026-09-22");
		expect(stripUid(` 🆔 ${UID}`)).toBe("");
		expect(stripUid(`a 🆔 ${UID} b`)).toBe("a b");
	});

	it("leaves other text alone", () => {
		expect(stripUid("Buy milk")).toBe("Buy milk");
		expect(stripUid("🆔 restask-nope")).toBe("🆔 restask-nope");
	});
});
