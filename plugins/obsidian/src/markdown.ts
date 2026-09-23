/**
 * Faithful TypeScript port of the Markdown grammar (docs/spec/markdown.md §6.1–§6.2).
 *
 * Pure string → data port of the Rust `markdown::parser` module: same regexes, same
 * codepoints, same canonical metadata tail order. No Obsidian API, no I/O, no wall
 * clock — unit-testable under Vitest (AGENTS.md §3).
 */

/** Priority keywords in canonical order (§3.2 / §7 section order). */
export const PRIORITIES = ["highest", "high", "medium", "low", "lowest"] as const;

/** Keyword form of the §3.2 priority table. */
export type Priority = (typeof PRIORITIES)[number];

/** Priority emoji, exactly the §3.2 codepoints. */
export const PRIORITY_EMOJI: Readonly<Record<Priority, string>> = {
	highest: "🔺",
	high: "⏫",
	medium: "🔼",
	low: "🔽",
	lowest: "⏬",
};

const PRIORITY_BY_EMOJI: ReadonlyMap<string, Priority> = new Map(
	PRIORITIES.map((p) => [PRIORITY_EMOJI[p], p]),
);

/** Canonical metadata tail order (§6.1): `<priority>` (first of the five present), then these. */
export const CANONICAL_TAIL_ORDER: readonly string[] = ["🛫", "⏳", "📅", "✅", "➕", "🆔"];

/** Default `done_heading` (§6.2, vault config default). */
export const DEFAULT_DONE_HEADING = "Done";

/** A due/start/scheduled value: date-only, or date + floating local `HH:MM` (§3.3). */
export type When = { kind: "date"; date: string } | { kind: "datetime"; date: string; time: string };

/** Per-line task fields (§6.1); `undefined` = token absent or value invalid (tolerated). */
export interface TaskDraft {
	uid: string | undefined;
	text: string;
	checked: boolean;
	priority: Priority | undefined;
	due: When | undefined;
	start: When | undefined;
	scheduled: When | undefined;
	created: string | undefined;
	completedOn: string | undefined;
}

/** A single line matched by the §6.1 grammar. */
export interface TaskLine {
	indentChars: number;
	marker: string;
	draft: TaskDraft;
}

/** A task line with file-level context (§6.2). */
export interface ParsedTask {
	lineNo: number;
	indentChars: number;
	raw: string;
	draft: TaskDraft;
	inDoneRegion: boolean;
	heading: string | undefined;
}

/** Whole-file parse result: never fails; non-task lines are simply absent (§6.2). */
export interface ParsedFile {
	tasks: ParsedTask[];
	doneHeadingLine: number | undefined;
	/** Matched metadata tokens whose value failed validation (§6.2 "anomaly counted"). */
	anomalies: number;
}

/** The subset of vault config the parser needs (§6.2 `done_heading`). */
export interface ParseCfg {
	doneHeading: string;
}

// §6.1 line grammar — only single-line, unordered-list checkboxes are tasks.
const LINE_RE = /^(?<indent>[ \t]*)(?<marker>[-*+])[ \t]+\[(?<check>[ xX])\][ \t]+(?<body>.*)$/;

// §6.1 metadata tokens (order-insensitive parse; priority must be a standalone token).
const PRIORITY_RE = /(^|[ \t])([🔺⏫🔼🔽⏬])(?=$|[ \t])/gu;
const DUE_RE = /📅[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)/gu;
const START_RE = /🛫[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)/gu;
const SCHEDULED_RE = /⏳[ \t]+(\d{4}-\d{2}-\d{2}(?:[ \t]+\d{2}:\d{2})?)/gu;
const COMPLETED_RE = /✅[ \t]+(\d{4}-\d{2}-\d{2})/gu;
const CREATED_RE = /➕[ \t]+(\d{4}-\d{2}-\d{2})/gu;
const UID_RE = /🆔[ \t]+(taskres-[0-9a-z]{26})/gu;

// Crockford base32: the ULID alphabet excludes i, l, o, u (§3.1).
const ULID_BODY_RE = /^[0-9a-hjkmnp-tv-z]{26}$/;

// §6.2 file-level shapes.
const HEADING_RE = /^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$/;
const FENCE_RE = /^\s*(`{3,}|~{3,})/;

interface Span {
	start: number;
	end: number;
}

function collect(body: string, re: RegExp, group: number): { value: string | undefined; spans: Span[] } {
	re.lastIndex = 0;
	const spans: Span[] = [];
	let value: string | undefined;
	for (let m = re.exec(body); m !== null; m = re.exec(body)) {
		spans.push({ start: m.index, end: m.index + m[0].length });
		if (value === undefined) value = m[group];
	}
	return { value, spans };
}

/** All matched token spans are pairwise disjoint (each kind starts with a distinct emoji). */
function stripSpans(body: string, spans: Span[]): string {
	let out = "";
	let pos = 0;
	for (const s of [...spans].sort((a, b) => a.start - b.start)) {
		out += body.slice(pos, s.start);
		pos = s.end;
	}
	return out + body.slice(pos);
}

/** `text` = body minus token spans, internal space/tab runs collapsed, both ends trimmed. */
function normalizeText(stripped: string): string {
	return stripped.replace(/[ \t]+/g, " ").trim();
}

function validDate(s: string): boolean {
	const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
	if (m === null) return false;
	const year = Number(m[1]);
	const month = Number(m[2]);
	const day = Number(m[3]);
	if (month < 1 || month > 12 || day < 1) return false;
	const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
	const daysInMonth = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
	return day <= daysInMonth;
}

function validTime(s: string): boolean {
	const m = /^(\d{2}):(\d{2})$/.exec(s);
	return m !== null && Number(m[1]) < 24 && Number(m[2]) < 60;
}

/** Validates a captured date/datetime token value; calendar-invalid → undefined (tolerated). */
export function parseWhenValue(value: string): When | undefined {
	const parts = value.split(/[ \t]+/);
	if (!validDate(parts[0])) return undefined;
	if (parts.length === 1) return { kind: "date", date: parts[0] };
	if (parts.length === 2 && validTime(parts[1])) {
		return { kind: "datetime", date: parts[0], time: parts[1] };
	}
	return undefined;
}

function emptyDraft(): TaskDraft {
	return {
		uid: undefined,
		text: "",
		checked: false,
		priority: undefined,
		due: undefined,
		start: undefined,
		scheduled: undefined,
		created: undefined,
		completedOn: undefined,
	};
}

/** Parses one line against the §6.1 grammar; non-task lines (including `1. [ ]`, `-[ ]`) → undefined. */
export function parseLine(line: string): TaskLine | undefined {
	const withoutEnding = line.replace(/\r?\n$|\r$/, "");
	const m = LINE_RE.exec(withoutEnding);
	if (m === null || m.groups === undefined) return undefined;
	const body = m.groups["body"];

	const due = collect(body, DUE_RE, 1);
	const start = collect(body, START_RE, 1);
	const scheduled = collect(body, SCHEDULED_RE, 1);
	const completed = collect(body, COMPLETED_RE, 1);
	const created = collect(body, CREATED_RE, 1);
	const uid = collect(body, UID_RE, 1);
	const priority = collect(body, PRIORITY_RE, 2);

	const dueValue = due.value !== undefined ? parseWhenValue(due.value) : undefined;
	const startValue = start.value !== undefined ? parseWhenValue(start.value) : undefined;
	const scheduledValue = scheduled.value !== undefined ? parseWhenValue(scheduled.value) : undefined;
	const createdValue = created.value !== undefined && validDate(created.value) ? created.value : undefined;
	const completedValue =
		completed.value !== undefined && validDate(completed.value) ? completed.value : undefined;
	const uidValue =
		uid.value !== undefined && ULID_BODY_RE.test(uid.value.slice("taskres-".length)) ? uid.value : undefined;

	let anomalies = 0;
	if (due.value !== undefined && dueValue === undefined) anomalies++;
	if (start.value !== undefined && startValue === undefined) anomalies++;
	if (scheduled.value !== undefined && scheduledValue === undefined) anomalies++;
	if (created.value !== undefined && createdValue === undefined) anomalies++;
	if (completed.value !== undefined && completedValue === undefined) anomalies++;
	if (uid.value !== undefined && uidValue === undefined) anomalies++;

	return {
		indentChars: m.groups["indent"].length,
		marker: m.groups["marker"],
		draft: {
			uid: uidValue,
			text: normalizeText(
				stripSpans(body, [
					...due.spans,
					...start.spans,
					...scheduled.spans,
					...completed.spans,
					...created.spans,
					...uid.spans,
					...priority.spans,
				]),
			),
			checked: m.groups["check"] === "x" || m.groups["check"] === "X",
			priority: priority.value !== undefined ? PRIORITY_BY_EMOJI.get(priority.value) : undefined,
			due: dueValue,
			start: startValue,
			scheduled: scheduledValue,
			created: createdValue,
			completedOn: completedValue,
		},
	};
}

/**
 * Pure whole-file parse (§6.2): YAML frontmatter (a `---` at byte 0, terminated) and fenced
 * code blocks (``` or ~~~) are never tasks; the first heading whose text equals
 * `cfg.doneHeading` (case-sensitive, any ATX level) starts the done region to EOF.
 */
export function parse(contents: string, cfg: ParseCfg = { doneHeading: DEFAULT_DONE_HEADING }): ParsedFile {
	const lines = contents.split(/\r\n|\r|\n/);
	const tasks: ParsedTask[] = [];
	let anomalies = 0;
	let doneHeadingLine: number | undefined;
	let lastHeading: string | undefined;
	let inFence: string | undefined;
	let idx = 0;

	if (lines[0] === "---") {
		const close = lines.indexOf("---", 1);
		if (close !== -1) idx = close + 1;
	}

	for (; idx < lines.length; idx++) {
		const line = lines[idx];
		if (inFence !== undefined) {
			const closing = FENCE_RE.exec(line);
			if (closing !== null && closing[1][0] === inFence) inFence = undefined;
			continue;
		}
		const opening = FENCE_RE.exec(line);
		if (opening !== null) {
			inFence = opening[1][0];
			continue;
		}
		const heading = HEADING_RE.exec(line);
		if (heading !== null) {
			lastHeading = heading[1];
			if (doneHeadingLine === undefined && heading[1] === cfg.doneHeading) {
				doneHeadingLine = idx + 1;
			}
			continue;
		}
		const task = parseLine(line);
		if (task === undefined) continue;
		tasks.push({
			lineNo: idx + 1,
			indentChars: task.indentChars,
			raw: line,
			draft: task.draft,
			inDoneRegion: doneHeadingLine !== undefined,
			heading: lastHeading,
		});
	}

	return { tasks, doneHeadingLine, anomalies };
}

/**
 * Nearest-ancestor checkbox by indent (§6.2): ancestors without a UID yield undefined
 * (unknown/unregistered ancestor → child treated as root, never transitive).
 */
export function linkParents(tasks: ParsedTask[]): (string | undefined)[] {
	const parents: (string | undefined)[] = [];
	const stack: { indent: number; uid: string | undefined }[] = [];
	for (const task of tasks) {
		while (stack.length > 0 && stack[stack.length - 1].indent >= task.indentChars) stack.pop();
		const top = stack[stack.length - 1];
		parents.push(top !== undefined ? top.uid : undefined);
		stack.push({ indent: task.indentChars, uid: task.draft.uid });
	}
	return parents;
}
