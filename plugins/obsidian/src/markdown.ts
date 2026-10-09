/**
 * Faithful TypeScript port of the Markdown grammar (docs/spec/markdown.md §6.1–§6.2).
 *
 * Pure string → data port of the Rust `markdown::parser` module: same regexes, same
 * codepoints, same canonical metadata tail order. No Obsidian API, no I/O, no wall
 * clock — unit-testable under Vitest.
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
export const CANONICAL_TAIL_ORDER: readonly string[] = ["🔁", "🛫", "⏳", "📅", "✅", "➕", "📁", "🆔"];

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
	/** `true` when the line asks for its creation date: a `➕` standing alone, without a date (§6.1). */
	wantsCreated: boolean;
	completedOn: string | undefined;
	/** `🔁` rule in the vault spelling, as written (e.g. `every 2 weeks on Monday`). */
	recurrence: string | undefined;
	/** `📁` value as a list slug: the calendar the line names (§6.1). It says where a task of the TODO.md view lives (§7.5). */
	list: string | undefined;
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
// A `➕` on its own, no date behind it: the request for the creation date.
export const BARE_CREATED_RE = /(^|[ \t])➕(?=$|[ \t])(?![ \t]+\d{4}-\d{2}-\d{2})/gu;
// A counted UID (§3.1) — a device tag and a number, as a whole word (`a42`) — or a long
// one in full. The engine's `UID_PATTERN`.
const UID_RE = /🆔[ \t]+((?:restask|taskres)-[0-9a-z]{26}|[a-z]{1,4}[1-9][0-9]{0,14}(?![0-9A-Za-z_]))/gu;
const COUNTED_UID_RE = /^([a-z]{1,4})([1-9][0-9]{0,14})$/;
// A calendar's name: one word of letters and digits with single hyphens inside.
const LIST_RE = /📁[ \t]+([0-9A-Za-z]+(?:-[0-9A-Za-z]+)*)/gu;

// `restask-` and the legacy `taskres-` prefix are both 8 characters long.
const UID_PREFIX_LENGTH = "restask-".length;

// Crockford base32: the ULID alphabet excludes i, l, o, u (§3.1).
const ULID_BODY_RE = /^[0-9a-hjkmnp-tv-z]{26}$/;

// §6.2 file-level shapes.
const HEADING_RE = /^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$/;
const FENCE_RE = /^\s*(`{3,}|~{3,})/;

/** A half-open range of UTF-16 offsets in a line or body. */
export interface Span {
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
		wantsCreated: false,
		completedOn: undefined,
		recurrence: undefined,
		list: undefined,
	};
}

const UNIT_RE = /^(?:minutes?|hours?|days?|weeks?|months?|years?)$/;
const WEEKDAY_RE = /^(?:mon(?:day)?|tue(?:sday)?|wed(?:nesday)?|thu(?:rsday)?|fri(?:day)?|sat(?:urday)?|sun(?:day)?)$/;
const ORDINAL_RE = /^(?:[1-9]|[12][0-9]|3[01])(?:st|nd|rd|th)?$/;

/**
 * Length of the repeat rule at the start of `text` in the vault spelling (§3.6), or
 * undefined when `text` does not start with one. Mirrors `Recurrence::from_text`:
 * `every [N] <unit>` | `every weekday`, then optional `on …` (weekdays for weeks; days of
 * the month or `<nth|last> <weekday>` for months), `for N times`, `until YYYY-MM-DD`.
 */
export function recurrenceLength(text: string): number | undefined {
	const words: { text: string; end: number }[] = [];
	const re = /[^ \t]+/g;
	for (let m = re.exec(text); m !== null; m = re.exec(text)) {
		const word = m[0].replace(/,+$/, "");
		if (word !== "") words.push({ text: word.toLowerCase(), end: m.index + word.length });
	}
	const w = (i: number): string | undefined => words[i]?.text;
	if (w(0) !== "every") return undefined;
	let at = 1;
	if (w(at) === "weekday" || w(at) === "weekdays") {
		at += 1;
	} else {
		if (/^[0-9]+$/.test(w(at) ?? "")) {
			if (Number(w(at)) === 0) return undefined;
			at += 1;
		}
		const unit = w(at);
		if (unit === undefined || !UNIT_RE.test(unit)) return undefined;
		at += 1;
		const weekly = unit.startsWith("week");
		const monthly = unit.startsWith("month");
		if (w(at) === "on" && (weekly || monthly)) {
			let next = at + 1;
			for (;;) {
				while (w(next) === "the" || w(next) === "and") next += 1;
				const first = w(next);
				if (first === undefined) break;
				const second = w(next + 1);
				if (weekly) {
					if (!WEEKDAY_RE.test(first)) break;
					next += 1;
				} else {
					const ordinal = first === "last" || ORDINAL_RE.test(first);
					if (!ordinal) break;
					if (second !== undefined && WEEKDAY_RE.test(second)) next += 2;
					else if (first === "last" && second === "day") next += 2;
					else if (first !== "last") next += 1;
					else break;
				}
				at = next;
			}
		}
	}
	if (w(at) === "for" && /^[0-9]+$/.test(w(at + 1) ?? "") && /^times?$/.test(w(at + 2) ?? "")) at += 3;
	if (w(at) === "until" && validDate(w(at + 1) ?? "")) at += 2;
	return words[at - 1].end;
}

/** The `🔁` token (§6.1): the emoji, whitespace, and exactly the words of the rule. */
function collectRecurrence(body: string): { value: string | undefined; spans: Span[] } {
	const at = body.indexOf("🔁");
	if (at < 0) return { value: undefined, spans: [] };
	const after = body.slice(at + "🔁".length);
	const rule = after.replace(/^[ \t]+/, "");
	const gap = after.length - rule.length;
	if (gap === 0) return { value: undefined, spans: [] };
	const length = recurrenceLength(rule);
	if (length === undefined) return { value: undefined, spans: [] };
	const start = at + "🔁".length + gap;
	return { value: rule.slice(0, length), spans: [{ start: at, end: start + length }] };
}

/** Offset at which the body of a task line starts (§6.1), or undefined when the line is not a task. */
export function bodyStart(line: string): number | undefined {
	const m = LINE_RE.exec(line);
	if (m === null || m.groups === undefined) return undefined;
	return m[0].length - m.groups["body"].length;
}

/** Offset of the checkbox character (` `, `x`, `X`) of a task line (§6.1), or undefined when the line is not a task. */
export function checkOffset(line: string): number | undefined {
	const m = LINE_RE.exec(line);
	if (m === null || m.groups === undefined) return undefined;
	return line.indexOf("[", m.groups["indent"].length) + 1;
}

/** `true` when `value`, as `UID_RE` matched it, is a UID (§3.1): a counted one, or a long one whose body is a ULID. */
function validUid(value: string): boolean {
	return COUNTED_UID_RE.test(value) || ULID_BODY_RE.test(value.slice(UID_PREFIX_LENGTH));
}

/**
 * The token that gives `body` its UID (§6.1): the first `🆔` match, and only when its
 * value is a valid UID — the same token `parseLine` reads the `uid` field from.
 */
export function uidSpan(body: string): Span | undefined {
	UID_RE.lastIndex = 0;
	const m = UID_RE.exec(body);
	if (m === null || !validUid(m[1])) return undefined;
	return { start: m.index, end: m.index + m[0].length };
}

/** The UIDs `text` — a line, or a whole note — spells behind a `🆔`, in order. */
export function uidsIn(text: string): string[] {
	return [...text.matchAll(UID_RE)].map((m) => m[1]).filter(validUid);
}

/**
 * `true` when a `🆔` token of `text` spells `uid`. A token is the whole word: `a4` is
 * not in a note because `a42` is (the engine's `uid_tokens`).
 */
export function hasUid(text: string, uid: string): boolean {
	return text.includes(uid) && uidsIn(text).includes(uid);
}

/** The device tag and the number of a counted UID (§3.1); undefined for a long one. */
export function countedUid(uid: string): { tag: string; number: number } | undefined {
	const m = COUNTED_UID_RE.exec(uid);
	return m === null ? undefined : { tag: m[1], number: Number(m[2]) };
}

/**
 * The order of two UIDs (§3.1, the engine's `Ord`): creation order as far as a UID tells
 * it — the long ones first, as text; then the counted ones by number, then by tag.
 * Negative when `a` comes first.
 */
export function uidOrder(a: string, b: string): number {
	const text = (x: string, y: string): number => (x < y ? -1 : x > y ? 1 : 0);
	const [ca, cb] = [countedUid(a), countedUid(b)];
	if (ca === undefined || cb === undefined) return ca === cb ? text(a, b) : ca === undefined ? -1 : 1;
	return ca.number - cb.number || text(ca.tag, cb.tag);
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
	const bareCreated = collect(body, BARE_CREATED_RE, 0);
	const uid = collect(body, UID_RE, 1);
	const list = collect(body, LIST_RE, 1);
	const priority = collect(body, PRIORITY_RE, 2);
	const recurrence = collectRecurrence(body);

	const dueValue = due.value !== undefined ? parseWhenValue(due.value) : undefined;
	const startValue = start.value !== undefined ? parseWhenValue(start.value) : undefined;
	const scheduledValue = scheduled.value !== undefined ? parseWhenValue(scheduled.value) : undefined;
	const createdValue = created.value !== undefined && validDate(created.value) ? created.value : undefined;
	const completedValue =
		completed.value !== undefined && validDate(completed.value) ? completed.value : undefined;
	const uidValue = uid.value !== undefined && validUid(uid.value) ? uid.value : undefined;

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
					...bareCreated.spans,
					...uid.spans,
					...list.spans,
					...priority.spans,
					...recurrence.spans,
				]),
			),
			checked: m.groups["check"] === "x" || m.groups["check"] === "X",
			priority: priority.value !== undefined ? PRIORITY_BY_EMOJI.get(priority.value) : undefined,
			due: dueValue,
			start: startValue,
			scheduled: scheduledValue,
			created: createdValue,
			wantsCreated: bareCreated.spans.length > 0,
			completedOn: completedValue,
			recurrence: recurrence.value,
			// The shape the token has is a slug's but for its case.
			list: list.value?.toLowerCase(),
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
 * (unknown/unregistered ancestor → child treated as root, never transitive). Nesting
 * never crosses a heading, and tasks in the done region have no parent.
 */
export function linkParents(tasks: ParsedTask[]): (string | undefined)[] {
	const parents: (string | undefined)[] = [];
	let stack: { indent: number; uid: string | undefined }[] = [];
	let section: string | undefined | null = null;
	for (const task of tasks) {
		if (section !== task.heading) {
			stack = [];
			section = task.heading;
		}
		if (task.inDoneRegion) {
			parents.push(undefined);
			continue;
		}
		while (stack.length > 0 && stack[stack.length - 1].indent >= task.indentChars) stack.pop();
		const top = stack[stack.length - 1];
		parents.push(top !== undefined ? top.uid : undefined);
		stack.push({ indent: task.indentChars, uid: task.draft.uid });
	}
	return parents;
}
