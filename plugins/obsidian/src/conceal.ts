/**
 * Where a task keeps its `🆔` token (docs/spec/integrations.md §15.5).
 *
 * The token stays in the file — it is the task's identity (§3.1) — but the plugin does
 * not show it. This module only locates it: pure strings in, offsets out. No Obsidian
 * API, no CodeMirror.
 */

import { bodyStart, uidSpan } from "./markdown";

/** The hidden part of a task line, in UTF-16 offsets of that line. */
export interface UidToken {
	/** Where the line's body starts (right after the checkbox). */
	bodyStart: number;
	/** Start of the hidden range: the token and the one blank before it. */
	start: number;
	/** End of the hidden range: the end of the UID. */
	end: number;
}

function blanksBefore(text: string, at: number, floor: number): number {
	let start = at;
	while (start > floor && (text[start - 1] === " " || text[start - 1] === "\t")) start--;
	return start;
}

/**
 * The token that gives a task line its UID, with the blank in front of it, or undefined
 * when the line is not a task or carries no valid UID. One blank only — the separator
 * registration writes: were every blank hidden, a blank typed at the visible end of the
 * line would vanish into the range and take the cursor with it. A token on any other
 * line, a malformed one, and a second one on the same line are not hidden: they are not
 * a task's identity, and the user should see them.
 */
export function uidToken(line: string): UidToken | undefined {
	const body = bodyStart(line);
	if (body === undefined) return undefined;
	const span = uidSpan(line.slice(body));
	if (span === undefined) return undefined;
	const at = body + span.start;
	const blank = at > body && (line[at - 1] === " " || line[at - 1] === "\t");
	return { bodyStart: body, start: blank ? at - 1 : at, end: body + span.end };
}

/** `text` — the rendered text of a task item — without its UID token (reading view). */
export function stripUid(text: string): string {
	const span = uidSpan(text);
	if (span === undefined) return text;
	return text.slice(0, blanksBefore(text, span.start, 0)) + text.slice(span.end);
}

const SEAL_LINE = /^restask-render: [0-9a-f]{16}$/;

/**
 * Index of the line that holds a view's seal (docs/spec/markdown.md §7.2) in the
 * frontmatter block of `lines`, or -1 when the file has none. The seal is the engine's
 * bookkeeping, not something to read or edit, so the editor does not show it.
 */
export function sealLine(lines: readonly string[]): number {
	if (lines[0] !== "---") return -1;
	for (let i = 1; i < lines.length && lines[i] !== "---"; i++) {
		if (SEAL_LINE.test(lines[i])) return i;
	}
	return -1;
}
