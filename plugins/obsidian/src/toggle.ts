/**
 * Task toggling as a pure document transformation (docs/spec/integrations.md §15.3).
 *
 * The plugin only ever edits Markdown: the daemon derives everything else from the
 * vault. Completing a task flips its checkbox, stamps `✅ <today>` and moves the line
 * directly under the done heading (newest on top); reopening removes the stamp and
 * moves the line back to the bottom of the active list. Apart from that, the line and
 * every other line are preserved byte for byte.
 *
 * No Obsidian API, no I/O, no wall clock — unit-testable under Vitest.
 */

import { parse, parseLine } from "./markdown";

/** Result of toggling the task on one line. */
export interface ToggleResult {
	/** The whole document after the toggle (lines joined with `\n`). */
	doc: string;
	/** 0-based line the toggled task now sits on. */
	line: number;
	/** `true` when the task was completed, `false` when it was reopened. */
	completed: boolean;
}

/** Sets the checkbox of a §6.1 task line, preserving everything else byte-for-byte. */
export function flipCheck(raw: string, checked: boolean): string {
	return raw.replace(/^([ \t]*[-*+][ \t]+\[)([ xX])(\])/, (_m, pre: string, _c: string, post: string) =>
		`${pre}${checked ? "x" : " "}${post}`,
	);
}

/** `true` when the line carries a `✅ <date>` token. */
export function isStamped(raw: string): boolean {
	return /✅[ \t]+\d{4}-\d{2}-\d{2}/u.test(raw);
}

/** Inserts `✅ <today>` in canonical tail position: before ➕/📁/🆔 when present (§6.1). */
export function withCompletedToken(raw: string, today: string): string {
	if (isStamped(raw)) return raw;
	const insertion = `✅ ${today}`;
	// The calendar token only where it is part of the tail: right before the UID, or last.
	const anchors = [
		/➕[ \t]+\d{4}-\d{2}-\d{2}/u,
		/📁[ \t]+[0-9A-Za-z]+(?:-[0-9A-Za-z]+)*(?=[ \t]*(?:🆔|$))/u,
		/🆔[ \t]+(?:(?:restask|taskres)-|[a-z]{1,4}[1-9])/u,
	];
	const anchor = anchors.map((token) => raw.search(token)).find((at) => at >= 0) ?? -1;
	if (anchor >= 0) {
		return `${raw.slice(0, anchor).replace(/[ \t]+$/, "")} ${insertion} ${raw.slice(anchor)}`;
	}
	return `${raw.replace(/[ \t]+$/, "")} ${insertion}`;
}

/** Removes the `✅ <date>` token. */
export function withoutCompletedToken(raw: string): string {
	return raw.replace(/[ \t]*✅[ \t]+\d{4}-\d{2}-\d{2}/u, "").replace(/[ \t]+$/, "");
}

/** A line that moves between regions leaves its parent's subtree: drop its indentation. */
function unindent(raw: string): string {
	return raw.replace(/^[ \t]+/, "");
}

/**
 * Toggles the task on `lineIdx` (0-based). Returns `undefined` when that line is not a
 * task line. `doneHeading` must equal the vault's `done_heading`.
 */
export function toggleDone(doc: string, lineIdx: number, today: string, doneHeading: string): ToggleResult | undefined {
	const lines = doc.split("\n");
	const raw = lines[lineIdx];
	if (raw === undefined) return undefined;
	const task = parseLine(raw);
	if (task === undefined) return undefined;

	const before = parse(doc, { doneHeading });
	const here = before.tasks.find((t) => t.lineNo === lineIdx + 1);
	const inDoneRegion = here !== undefined && here.inDoneRegion;
	const completing = !task.draft.checked;
	const edited = completing ? withCompletedToken(flipCheck(raw, true), today) : withoutCompletedToken(flipCheck(raw, false));

	// Already where it belongs: edit in place.
	if (completing === inDoneRegion || here === undefined) {
		lines[lineIdx] = edited;
		return { doc: lines.join("\n"), line: lineIdx, completed: completing };
	}

	const moved = unindent(edited);
	lines.splice(lineIdx, 1);
	const after = parse(lines.join("\n"), { doneHeading });

	if (completing) {
		if (after.doneHeadingLine !== undefined) {
			// `doneHeadingLine` is 1-based: this index is the line right below the heading.
			lines.splice(after.doneHeadingLine, 0, moved);
			return { doc: lines.join("\n"), line: after.doneHeadingLine, completed: true };
		}
		// No done heading yet: create a level-3 one at the end of the file, separated
		// from what precedes it by exactly one blank line.
		if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
		if (lines.length === 0 || lines[lines.length - 1].trim() !== "") lines.push("");
		lines.push(`### ${doneHeading}`, moved);
		return { doc: `${lines.join("\n")}\n`, line: lines.length - 1, completed: true };
	}

	// Reopening: bottom of the active list — after the last active task; else above the
	// done heading (and the blank lines before it).
	const active = after.tasks.filter((t) => !t.inDoneRegion);
	let at: number;
	if (active.length > 0) {
		at = active[active.length - 1].lineNo;
	} else if (after.doneHeadingLine !== undefined) {
		at = after.doneHeadingLine - 1;
		while (at > 0 && lines[at - 1].trim() === "") at -= 1;
	} else {
		at = lines.length;
	}
	lines.splice(at, 0, moved);
	return { doc: lines.join("\n"), line: at, completed: false };
}
