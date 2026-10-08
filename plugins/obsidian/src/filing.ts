/**
 * The daemon's local edits, made on the device a task is edited on
 * (docs/spec/integrations.md §15.6).
 *
 * The daemon gives a new task line its `➕`/`🆔` tokens, brings a line in line with its
 * checkbox, and keeps the TODO.md view filed — but only where it runs: a phone waits for
 * the file sync to go there and back. This module makes the same edits as pure line
 * transformations, so the plugin can apply them at once, offline. What it produces is
 * what the daemon accepts as it is.
 *
 * No Obsidian API, no I/O, no wall clock, no randomness of its own — unit-testable under
 * Vitest.
 */

import {
	BARE_CREATED_RE,
	PRIORITIES,
	PRIORITY_EMOJI,
	checkOffset,
	parse,
	parseLine,
	type ParsedTask,
	type Priority,
	type TaskDraft,
	type When,
} from "./markdown";
import { flipCheck, isStamped, toggleDone, withCompletedToken, withoutCompletedToken } from "./toggle";

const PRIORITY_NAME: Readonly<Record<Priority, string>> = {
	highest: "Highest",
	high: "High",
	medium: "Medium",
	low: "Low",
	lowest: "Lowest",
};

const NO_PRIORITY_SECTION = "No Priority";
const DONE_SECTION = "Done";

/** Heading text of a priority's section in the view (§7). */
export function prioritySection(priority: Priority): string {
	return `${PRIORITY_EMOJI[priority]} ${PRIORITY_NAME[priority]} Priority`;
}

/** The priority a section of the view stands for (§7.4): defined for the heading text of a priority's section only. */
export function sectionPriority(heading: string | undefined): Priority | undefined {
	return PRIORITIES.find((priority) => prioritySection(priority) === heading);
}

/**
 * The priority a line takes from being moved under `heading` (§7.4): a priority
 * section gives its priority, `No Priority` takes it away. Undefined under any other
 * heading, which says nothing about the priority.
 */
function movedPriority(heading: string | undefined): { priority: Priority | undefined } | undefined {
	const priority = sectionPriority(heading);
	return priority !== undefined || heading === NO_PRIORITY_SECTION ? { priority } : undefined;
}

// The first standalone priority emoji of a line, with the blank in front of it (§6.1).
const PRIORITY_TOKEN = /(^|[ \t])([🔺⏫🔼🔽⏬])(?=$|[ \t])/u;

/**
 * A mirror line with its priority emoji replaced, or taken out when `priority` is
 * undefined; the rest of the line is not touched. Undefined when the line has no
 * priority emoji to work on.
 */
function withPriority(line: string, priority: Priority | undefined): string | undefined {
	const found = PRIORITY_TOKEN.exec(line);
	if (found === null) return undefined;
	const put = priority === undefined ? "" : `${found[1]}${PRIORITY_EMOJI[priority]}`;
	return `${line.slice(0, found.index)}${put}${line.slice(found.index + found[0].length)}`;
}

/**
 * How a view is laid out: TODO.md (§7), or the view a root note holds in its TODO
 * section (§7.6) — the same sections under headings of another rank, closed by the
 * vault's done heading.
 */
export interface Layout {
	/** The `#`s of the view's section headings. */
	hashes: string;
	/** Text of the heading the view's completed tasks stand under. */
	done: string;
	/** Whether a line of the view has the shape of a mirror line. */
	mirror: (raw: string) => boolean;
}

/** The view's sections in render order (§7). */
function sectionsOf(layout: Layout): readonly string[] {
	return [...PRIORITIES.map(prioritySection), NO_PRIORITY_SECTION, layout.done];
}

// Crockford base32, lowercase (§3.1).
const ALPHABET = "0123456789abcdefghjkmnpqrstvwxyz";
const TIME_DIGITS = 10;
const RANDOM_DIGITS = 16;

/**
 * A source of fresh task UIDs (§3.1): `restask-` + a ULID of `now()` milliseconds and 80
 * random bits taken from `randomBytes(16)`. Like the engine's generator it is monotonic:
 * a UID made in the same millisecond as the previous one (or after the clock stepped
 * back) is the previous one plus one, so tasks registered together keep their order.
 */
export function uidGenerator(now: () => number, randomBytes: (count: number) => Uint8Array): () => string {
	let lastTime = -1;
	let lastRandom: number[] = [];
	return () => {
		const time = Math.max(0, Math.floor(now()));
		let random: number[];
		if (time <= lastTime) {
			random = [...lastRandom];
			let at = RANDOM_DIGITS - 1;
			while (at >= 0 && random[at] === ALPHABET.length - 1) random[at--] = 0;
			if (at >= 0) random[at] += 1;
		} else {
			lastTime = time;
			random = Array.from(randomBytes(RANDOM_DIGITS), (byte) => byte % ALPHABET.length);
		}
		lastRandom = random;
		let stamp = "";
		for (let rest = lastTime, i = 0; i < TIME_DIGITS; i++, rest = Math.floor(rest / ALPHABET.length)) {
			stamp = ALPHABET[rest % ALPHABET.length] + stamp;
		}
		return `restask-${stamp}${random.map((digit) => ALPHABET[digit]).join("")}`;
	};
}

/**
 * `line` with the token that registers it (§6.4): `🆔 <uid()>`, appended behind
 * everything else. A creation date is not written — unless the line asks for one with a
 * bare `➕`, which then gets `today` behind it where it stands. Undefined when there is
 * nothing to register: not a task line, no text yet, or a `🆔` already on the line — a
 * malformed one included, which is the daemon's to repair. `section` is the priority of
 * the view section the line was typed in (§7.4): a line that names no priority gets its
 * emoji, ahead of the UID. A calendar token the line ends in (§7.5) stays the last thing
 * before the UID, written as the engine writes it: the calendar's slug.
 */
export function registeredLine(line: string, today: string, uid: () => string, section?: Priority): string | undefined {
	const task = parseLine(line);
	if (task === undefined || task.draft.text === "" || line.includes("🆔")) return undefined;
	const ranked = section !== undefined && task.draft.priority === undefined ? ` ${PRIORITY_EMOJI[section]}` : "";
	const dated = (task.draft.wantsCreated && task.draft.created === undefined ? withCreated(line, today) : line).replace(/[ \t]+$/, "");
	const calendar = TRAILING_CALENDAR.exec(dated);
	const head = calendar === null ? dated : dated.slice(0, calendar.index);
	const named = calendar === null ? "" : ` 📁 ${calendar[1].toLowerCase()}`;
	return `${head}${ranked}${named} 🆔 ${uid()}`;
}

// A calendar token (§6.1) at the end of a line, with the blanks in front of it.
const TRAILING_CALENDAR = /[ \t]+📁[ \t]+([0-9A-Za-z]+(?:-[0-9A-Za-z]+)*)$/u;

/** `line` with `date` written behind its first bare `➕` (§6.4); further bare ones are left for the daemon to drop. */
function withCreated(line: string, date: string): string {
	BARE_CREATED_RE.lastIndex = 0;
	const found = BARE_CREATED_RE.exec(line);
	if (found === null) return line;
	const end = found.index + found[0].length;
	return `${line.slice(0, end)} ${date}${line.slice(end)}`;
}

/** `true` when `raw` has the shape of a rendered mirror line: a wikilink right before the `🆔` token (§7.1). */
function looksLikeMirror(raw: string): boolean {
	const at = raw.lastIndexOf("🆔");
	if (at < 0) return false;
	const before = raw.slice(0, at).trimEnd();
	return before.endsWith("]]") && before.includes("[[");
}

/** TODO.md's layout (§7). */
const INBOX: Layout = { hashes: "##", done: DONE_SECTION, mirror: looksLikeMirror };

/**
 * `true` when `raw` is a mirror line as a render writes one (§7), ticked since or not:
 * with a priority, and ending in the link to its note — `[[<stem>|<stem>]]` or
 * `[[<stem>#<heading>|<stem>]]` — right before the `🆔` token. In a root note the
 * shape, not a mere wikilink at the end of the text, tells a mirror line from a task of
 * the note (§7.6; the engine's `is_mirror_shaped`).
 */
export function isMirrorShaped(raw: string): boolean {
	const draft = parseLine(raw)?.draft;
	if (draft?.uid === undefined) return false;
	const tail = new RegExp(`\\]\\][ \\t]*🆔[ \\t]*${draft.uid}[ \\t]*$`, "u").exec(raw);
	if (tail === null) return false;
	const before = raw.slice(0, tail.index);
	const open = before.lastIndexOf("[[");
	const bar = before.lastIndexOf("|");
	if (open < 0 || bar < open) return false;
	const stem = before.slice(open + 2, bar).split("#")[0];
	// The priority stands in front of the link, where the render writes it.
	return stem !== "" && stem === before.slice(bar + 1) && !/[[\]]/.test(stem) && parseLine(before.slice(0, open))?.draft.priority !== undefined;
}

interface Heading {
	idx: number;
	text: string;
	/** Rank of the heading: the number of its `#`s. */
	level: number;
}

const HEADING_RE = /^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$/;
const FENCE_RE = /^\s*(`{3,}|~{3,})/;

/** Index of the first line after the frontmatter block (§5.1), 0 when there is none. */
function bodyStartLine(lines: readonly string[]): number {
	if (lines[0]?.trim() !== "---") return 0;
	for (let i = 1; i < lines.length; i++) if (lines[i].trim() === "---") return i + 1;
	return 0;
}

/** The headings of a note (§6.2): none in the frontmatter, none in a fenced block. */
function headings(lines: readonly string[]): Heading[] {
	const found: Heading[] = [];
	let inFence: string | undefined;
	for (let idx = bodyStartLine(lines); idx < lines.length; idx++) {
		const fence = FENCE_RE.exec(lines[idx]);
		if (inFence !== undefined) {
			if (fence !== null && fence[1][0] === inFence) inFence = undefined;
			continue;
		}
		if (fence !== null) {
			inFence = fence[1][0];
			continue;
		}
		const heading = HEADING_RE.exec(lines[idx]);
		if (heading !== null) found.push({ idx, text: heading[2], level: heading[1].length });
	}
	return found;
}

/**
 * `true` when the note's own frontmatter declares `restask-list-root` (§5.1) with a
 * name a list can be made of: a root note.
 */
export function declaresRoot(lines: readonly string[]): boolean {
	const end = bodyStartLine(lines);
	for (let i = 1; i < end - 1; i++) {
		const colon = lines[i].indexOf(":");
		if (colon >= 0 && lines[i].slice(0, colon).trim() === "restask-list-root") return /[0-9A-Za-z]/.test(lines[i].slice(colon + 1));
	}
	return false;
}

/** A view within the lines of its file. */
export interface View {
	/** Index of the view's first line: 0 in TODO.md, the line of the `TODO` heading in a root note. */
	from: number;
	/** Index of the first line after the view. */
	to: number;
	/** `true` for the view of a root note (§7.6), which is a section of the note. */
	root: boolean;
	/** Text of the heading that opens the view of a root note. */
	title?: string;
	layout: Layout;
}

/**
 * The view of a root note (§7.6, the engine's `section`): its TODO section — from the
 * first heading whose text is `TODO` (any letter case, rank 1–5) down to the next
 * heading of the same or a higher rank, which the done heading never is, or to the end
 * of the note. Undefined when the note has no such heading, or no frontmatter block
 * between two lines that are exactly `---` (the seal lives there).
 */
function rootView(lines: readonly string[], doneHeading: string): View | undefined {
	const body = bodyStartLine(lines);
	if (lines[0] !== "---" || lines[body - 1] !== "---") return undefined;
	const heads = headings(lines);
	const at = heads.findIndex((h) => h.level < 6 && h.text.toLowerCase() === TODO_HEADING);
	if (at < 0) return undefined;
	const head = heads[at];
	const next = heads.slice(at + 1).find((h) => h.level <= head.level && h.text !== doneHeading);
	return {
		from: head.idx,
		to: next?.idx ?? lines.length,
		root: true,
		title: head.text,
		layout: { hashes: "#".repeat(head.level + 1), done: doneHeading, mirror: isMirrorShaped },
	};
}

/**
 * The view the note holds: all of TODO.md (§7), the TODO section of a root note (§7.6),
 * undefined for any other note.
 */
export function viewOf(lines: readonly string[], note: NoteKind): View | undefined {
	if (note.inboxView) return { from: 0, to: lines.length, root: false, layout: note.layout ?? INBOX };
	return note.root === true ? rootView(lines, note.doneHeading) : undefined;
}

/** `true` when the line on `idx` is a line of the view — not the heading that opens the view of a root note. */
export function inView(view: View | undefined, idx: number): view is View {
	return view !== undefined && idx < view.to && (view.root ? idx > view.from : idx >= view.from);
}

/** Runs `edit` on the lines of `view` as a document of their own and puts what it returns back among `lines`. */
function within(lines: readonly string[], view: View, edit: (sub: string[]) => string[] | undefined): string[] | undefined {
	const out = edit(lines.slice(view.from, view.to));
	return out === undefined ? undefined : [...lines.slice(0, view.from), ...out, ...lines.slice(view.to)];
}

/** What a new line in a TODO section starts with (§15.7): an unchecked box, ready for the task's text. */
export const TASK_START = "- [ ] ";

const TODO_HEADING = "todo";
const LEVELLED_HEADING_RE = /^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$/;

/**
 * `true` when the line on `idx` (0-based) lies in the TODO section of its note (§15.7),
 * where a new line starts a task. In a note that is the lines below a heading whose text
 * is `TODO` (in any letter case) down to the next heading of the same or a higher level;
 * headings below it in rank stay inside. In the TODO.md view it is the whole body. The
 * done heading ends the section in both — what is written under it is a record, not a
 * new task — and the frontmatter and fenced blocks are never part of it (§6.2).
 */
export function inTodoSection(lines: readonly string[], idx: number, note: NoteKind): boolean {
	const start = bodyStartLine(lines);
	if (idx < start) return false;
	const done = note.inboxView ? DONE_SECTION : note.doneHeading;
	// The rank of the heading that opened the section; the view is one from its first line.
	let level: number | undefined = note.inboxView ? 0 : undefined;
	let inFence: string | undefined;
	for (let i = start; i < idx && i < lines.length; i++) {
		const fence = FENCE_RE.exec(lines[i]);
		if (inFence !== undefined) {
			if (fence !== null && fence[1][0] === inFence) inFence = undefined;
			continue;
		}
		if (fence !== null) {
			inFence = fence[1][0];
			continue;
		}
		const heading = LEVELLED_HEADING_RE.exec(lines[i]);
		if (heading === null) continue;
		if (heading[2] === done) return false;
		if (note.inboxView) continue;
		if (level === undefined) {
			if (heading[2].toLowerCase() === TODO_HEADING) level = heading[1].length;
		} else if (heading[1].length <= level) {
			level = heading[2].toLowerCase() === TODO_HEADING ? heading[1].length : undefined;
		}
	}
	return level !== undefined && inFence === undefined;
}

// The seal is a property of the view's frontmatter block (§7.2); nothing is written into the body for it.
const SEAL_PREFIX = "restask-render: ";

/** The digest a seal carries (§7.2): FNV-1a (64-bit) over the UTF-8 text, 16 lowercase hex digits. */
export function digest(text: string): string {
	let hash = 0xcbf29ce484222325n;
	for (const byte of new TextEncoder().encode(text)) {
		hash = ((hash ^ BigInt(byte)) * 0x100000001b3n) & 0xffffffffffffffffn;
	}
	return hash.toString(16).padStart(16, "0");
}

/** Index of the seal line: the first line of the frontmatter block that opens with the seal's key; -1 when there is none. */
function sealIndex(lines: readonly string[]): number {
	if (lines[0] !== "---") return -1;
	for (let i = 1; i < lines.length && lines[i] !== "---"; i++) {
		if (lines[i].startsWith(SEAL_PREFIX)) return i;
	}
	return -1;
}

/**
 * `true` when the view is exactly as some device rendered it (§7.2): the seal in its
 * frontmatter matches the rest of the text. Such a view holds no edit that still has to
 * reach a note; whatever the user changes afterwards breaks the seal.
 */
export function isSealed(lines: readonly string[], view?: View): boolean {
	const at = sealIndex(lines);
	if (at < 0 || !lines.includes("---", at + 1)) return false;
	return lines[at].slice(SEAL_PREFIX.length) === sealOf(lines, at, view);
}

/**
 * The digest that seals the view (§7.2): over the whole of TODO.md without its seal
 * line `at`; for the view of a root note over its TODO section alone (§7.6), so that
 * what the user writes in the rest of the note breaks nothing.
 */
function sealOf(lines: readonly string[], at: number, view?: View): string {
	if (view?.root !== true) return digest(lines.filter((_l, i) => i !== at).join("\n"));
	return digest(lines.slice(view.from, view.to).join("\n") + (view.to < lines.length ? "\n" : ""));
}

/**
 * The view sealed again after the plugin's own edits (§7.2). Only a view that has a
 * seal line gets one: a view rendered by a daemon that does not seal there stays as it
 * is. The caller vouches that every edit of the user's in it has reached its note.
 */
export function resealed(lines: readonly string[], view?: View): string[] {
	const at = sealIndex(lines);
	if (at < 0) return [...lines];
	const out = [...lines];
	out[at] = `${SEAL_PREFIX}${sealOf(lines, at, view)}`;
	return out;
}

/** The digest of a seal line that claims no render (§7.1). */
const NO_CLAIM = "0000000000000000";

/**
 * The view with its seal line claiming no render (§7.1). For a view the plugin took a
 * mirror line out of and does not seal in the same write: left with the seal of the
 * render it was, the missing line would read as the user's deletion of the task, and a
 * daemon that has the view before the note would delete it there. A view without a seal
 * line stays as it is.
 */
export function disclaimed(lines: readonly string[]): string[] {
	const at = sealIndex(lines);
	const out = [...lines];
	if (at >= 0) out[at] = `${SEAL_PREFIX}${NO_CLAIM}`;
	return out;
}

/**
 * `true` when the note's own frontmatter routes it (§5.1): a non-empty `restask-list` or
 * `restask-list-root`. Line-scanned like the engine does. A note routed only by a
 * `restask-list-root` further up its folder is for the caller to recognise.
 */
export function routesItself(lines: readonly string[]): boolean {
	const end = bodyStartLine(lines);
	for (let i = 1; i < end - 1; i++) {
		const colon = lines[i].indexOf(":");
		if (colon < 0) continue;
		const key = lines[i].slice(0, colon).trim();
		if ((key === "restask-list" || key === "restask-list-root") && lines[i].slice(colon + 1).trim() !== "") return true;
	}
	return false;
}

/**
 * Takes the line on `idx` out of the view, in place. A section of the view the line
 * leaves without content goes with it (the render omits empty sections); `Done` stays.
 */
function takeOut(out: string[], idx: number, layout: Layout): void {
	const heads = headings(out);
	const own = [...heads].reverse().find((h) => h.idx < idx);
	let from = idx;
	let count = 1;
	if (own !== undefined && own.text !== layout.done && sectionsOf(layout).includes(own.text)) {
		let end = heads.find((h) => h.idx > idx)?.idx ?? out.length;
		const rest = out.slice(own.idx + 1, end).filter((_l, i) => own.idx + 1 + i !== idx);
		if (rest.every((l) => l.trim() === "")) {
			// The file keeps its final line break.
			if (end === out.length && out[end - 1] === "") end -= 1;
			from = own.idx;
			count = end - own.idx;
		}
	}
	out.splice(from, count);
}

/** Sort key of a `Done` line (§7): completion date, then UID — the render lists both descending. */
function doneKey(line: string): string {
	const draft = parseLine(line)?.draft;
	return `${draft?.completedOn ?? ""} ${draft?.uid ?? ""}`;
}

/** Puts `line` into the section `target` of the view, in place, creating the section in render order when missing. */
function putIn(out: string[], line: string, target: string, layout: Layout): void {
	const sections = sectionsOf(layout);
	const heads = headings(out);
	const section = heads.find((h) => h.text === target);
	if (section !== undefined) {
		const end = heads.find((h) => h.idx > section.idx)?.idx ?? out.length;
		let at = section.idx + 1;
		for (let i = section.idx + 1; i < end; i++) {
			if (parseLine(out[i]) === undefined) continue;
			// `Done` is newest first; the other sections grow at the bottom.
			if (target === layout.done && doneKey(line) > doneKey(out[i])) break;
			at = i + 1;
		}
		out.splice(at, 0, line);
		return;
	}
	const rank = sections.indexOf(target);
	const later = heads.find((h) => sections.indexOf(h.text) > rank);
	if (later !== undefined) {
		out.splice(later.idx, 0, `${layout.hashes} ${target}`, line, "");
		return;
	}
	let at = out.length;
	if (at > 0 && out[at - 1] === "") at -= 1;
	const block = [`${layout.hashes} ${target}`, line];
	if (at > 0 && out[at - 1].trim() !== "") block.unshift("");
	out.splice(at, 0, ...block);
}

/**
 * The TODO.md view with the task on `idx` moved to the section the next render puts it
 * in (§7): an active task to its priority's section, or to `No Priority` when it is a task of
 * the view's own without a priority; a completed task of the view's own to `Done`,
 * newest first. An active line goes behind the section's last task. A missing section
 * is created in render order, and a section the line leaves empty goes with it (`Done`
 * stays). Undefined when the line stays where it is: it is in its section already, or
 * it is a mirror line the render would not show (no priority, or completed) — the
 * plugin never drops a line on that ground. `own` says the line is a task of the view
 * itself even though it looks like a mirror line. `view`: the view of a root note the
 * line is in (§7.6), when the lines are not TODO.md.
 */
export function refiled(lines: readonly string[], idx: number, own = false, view?: View): string[] | undefined {
	if (view?.root === true) return within(lines, view, (sub) => refiledIn(sub, idx - view.from, own, view.layout));
	return refiledIn(lines, idx, own, view?.layout ?? INBOX);
}

function refiledIn(lines: readonly string[], idx: number, own: boolean, layout: Layout): string[] | undefined {
	const task = parse(lines.join("\n")).tasks.find((t) => t.lineNo === idx + 1);
	if (task === undefined) return undefined;
	const mirror = !own && layout.mirror(lines[idx]);
	const target = task.draft.checked
		? mirror
			? undefined
			: layout.done
		: task.draft.priority !== undefined
			? prioritySection(task.draft.priority)
			: mirror
				? undefined
				: NO_PRIORITY_SECTION;
	if (target === undefined || task.heading === target) return undefined;
	const out = [...lines];
	const line = out[idx];
	takeOut(out, idx, layout);
	putIn(out, line, target, layout);
	return out;
}

/** The view without the line on `idx` (§7.1: a mirror line whose task was completed in its note). */
export function removed(lines: readonly string[], idx: number, view?: View): string[] {
	if (view?.root === true) return within(lines, view, (sub) => removed(sub, idx - view.from, { ...view, root: false })) ?? [...lines];
	const out = [...lines];
	takeOut(out, idx, view?.layout ?? INBOX);
	return out;
}

/**
 * A note with the task on `idx` brought in line with its checkbox (§6.4, "the checkbox
 * is the status"): checked outside the done region → stamped `✅ <today>` and moved
 * under the done heading; checked inside it without a date → stamped; unchecked inside
 * it → moved back to the bottom of the active list, the date dropped; unchecked with a
 * date → the date dropped. Undefined when the line is consistent already.
 */
export function statusRepaired(lines: readonly string[], idx: number, today: string, doneHeading: string): string[] | undefined {
	const task = parse(lines.join("\n"), { doneHeading }).tasks.find((t) => t.lineNo === idx + 1);
	if (task === undefined) return undefined;
	const stamped = isStamped(lines[idx]);
	const consistent = task.draft.checked ? task.inDoneRegion && stamped : !task.inDoneRegion && !stamped;
	if (consistent) return undefined;
	// The toggle of §15.3 makes exactly these edits when it flips the box: flip it back first.
	const before = [...lines];
	before[idx] = flipCheck(lines[idx], !task.draft.checked);
	return toggleDone(before.join("\n"), idx, today, doneHeading)?.doc.split("\n");
}

/** The kind of note a line is settled in. */
export interface NoteKind {
	/** The note is the engine's TODO.md view (§7). */
	inboxView: boolean;
	/** The vault's `done_heading` (§6.2). */
	doneHeading: string;
	/** The note declares `restask-list-root`: its TODO section is a view (§7.6). */
	root?: boolean;
	/** The layout of the view, when it is not TODO.md's. */
	layout?: Layout;
}

/** What settling one line comes to. */
export interface Settled {
	/** The note after the edit; absent when its text does not change. */
	lines?: string[];
	/** The UID of the task on the line, once it has one. */
	uid?: string;
	/** `true` for a mirror line of the view: what the user changed on it belongs in its source note (§7.1). */
	mirror?: boolean;
	/**
	 * Set for a checked mirror line of the view: the task with this UID is to be
	 * completed in its source note, and the line then leaves the view (§7.1).
	 */
	carry?: string;
	/**
	 * Set for a mirror line that was moved to another section (§7.4): the line as it
	 * arrived and as it is with the section's priority — the edit to carry to its note.
	 */
	moved?: { was: string; now: string };
}

/**
 * The task on `idx` (0-based) settled the way the daemon's local phase would (§15.6):
 * registered — in the TODO.md view with the priority of the section it was typed in
 * (§7.4); in a note, brought in line with its checkbox; in the view, stamped or unstamped
 * and filed in its section. Undefined when the line is no task (also by the
 * file-level rules of §6.2) or has no text yet. `own`: see {@link refiled}. `moved`
 * says the line was not edited where it stood but arrived where it is — pasted, or moved
 * there: in the view, a registered active line then takes the priority of the section it
 * arrived in instead of being filed back by its emoji (§7.4).
 */
export function settled(
	lines: readonly string[],
	idx: number,
	note: NoteKind,
	today: string,
	uid: () => string,
	own = false,
	moved = false,
): Settled | undefined {
	const view = note.inboxView ? undefined : viewOf(lines, note);
	if (inView(view, idx)) {
		// A line in the view of a root note is settled as a line of TODO.md is (§7.6).
		const kind: NoteKind = { inboxView: true, doneHeading: note.doneHeading, layout: view.layout };
		const result = settled(lines.slice(view.from, view.to), idx - view.from, kind, today, uid, own, moved);
		if (result?.lines === undefined) return result;
		return { ...result, lines: [...lines.slice(0, view.from), ...result.lines, ...lines.slice(view.to)] };
	}
	const layout = note.layout ?? INBOX;
	const tasks = parse(lines.join("\n"), { doneHeading: note.doneHeading }).tasks;
	const task = tasks.find((t) => t.lineNo === idx + 1);
	if (task === undefined || task.draft.text === "") return undefined;
	let out: string[] | undefined;
	const registered = registeredLine(lines[idx], today, uid, note.inboxView ? sectionPriority(task.heading) : undefined);
	if (registered !== undefined) {
		out = [...lines];
		out[idx] = registered;
	}
	let id = parseLine(registered ?? lines[idx])?.draft.uid;
	const mirror = note.inboxView && !own && layout.mirror(lines[idx]);
	if (id !== undefined && !mirror) {
		// A copied line gets its own UID; the first occurrence keeps the one they share (§6.4).
		const copies = tasks
			.filter((t) => t.draft.uid === id && !(note.inboxView && layout.mirror(t.raw)))
			.slice(1);
		for (const copy of copies) {
			const fresh = uid();
			out = out ?? [...lines];
			out[copy.lineNo - 1] = out[copy.lineNo - 1].replace(new RegExp(`🆔[ \\t]+${id}`, "u"), `🆔 ${fresh}`);
			if (copy.lineNo === idx + 1) id = fresh;
		}
	}
	if (!note.inboxView) {
		out = statusRepaired(out ?? lines, idx, today, note.doneHeading) ?? out;
		return { lines: out, uid: id };
	}
	// A registered active line that arrived under another section's heading takes that section's priority (§7.4).
	const target = moved && registered === undefined && id !== undefined && !task.draft.checked ? movedPriority(task.heading) : undefined;
	const reprioritized = target !== undefined && target.priority !== task.draft.priority;
	if (mirror) {
		// A checked mirror line is a completion to make in the note; an unchecked one is filed by its priority.
		if (task.draft.checked) return { uid: id, mirror: true, carry: id };
		const now = reprioritized ? withPriority(lines[idx], target.priority) : undefined;
		if (now === undefined) return { lines: refiledIn(lines, idx, false, layout), uid: id, mirror: true };
		const shown = [...lines];
		shown[idx] = now;
		// Where it was cut from the plugin has put it back, as it does a deleted mirror line (§7.1): the task keeps one line.
		// Bottom up: a line taken out below `idx` moves nothing above it, and above `idx` the index is not needed again.
		for (let i = shown.length - 1; i >= 0; i--) {
			if (i !== idx && layout.mirror(shown[i]) && parseLine(shown[i])?.draft.uid === id) takeOut(shown, i, layout);
		}
		return { lines: shown, uid: id, mirror: true, moved: { was: lines[idx], now } };
	}
	if (reprioritized) {
		const current = (out ?? lines)[idx];
		const draft = parseLine(current)?.draft;
		if (draft !== undefined) {
			out = [...(out ?? lines)];
			out[idx] = canonicalLine(current.slice(0, task.indentChars), current.trimStart()[0], { ...draft, priority: target.priority });
		}
	}
	const line = (out ?? lines)[idx];
	const dated = task.draft.checked ? withCompletedToken(line, today) : isStamped(line) ? withoutCompletedToken(line) : line;
	if (dated !== line) {
		out = [...(out ?? lines)];
		out[idx] = dated;
	}
	out = refiledIn(out ?? lines, idx, own, layout) ?? out;
	return { lines: out, uid: id };
}

/**
 * The heading a mirror line of `task` names in its link (§7): the task's nearest
 * heading — or, for a line in the view of a root note, the heading of the view, not the
 * section of it the task happens to be filed in (§7.6).
 */
function linkHeading(lines: readonly string[], task: ParsedTask, doneHeading: string): string | undefined {
	const view = declaresRoot(lines) ? rootView(lines, doneHeading) : undefined;
	return inView(view, task.lineNo - 1) ? view.title : task.heading;
}

/**
 * The line of the note on which the task `uid` lives, -1 when the note has no such
 * task. A mirror line in the view of a root note is not the task (§7.6).
 */
export function taskIndex(lines: readonly string[], uid: string, doneHeading: string): number {
	const view = declaresRoot(lines) ? rootView(lines, doneHeading) : undefined;
	return lines.findIndex((line, i) => parseLine(line)?.draft.uid === uid && !(inView(view, i) && view.layout.mirror(line)));
}

/**
 * The mirror line of a task that lives in a note (§7):
 * `- [ ] <text> <priority> <🔁?><🛫?><⏳?><📅?> [[<stem>#<heading>|<stem>]] 🆔 <uid>`.
 * Undefined when the view does not show the task: it has no UID or no priority, or it
 * is completed.
 */
export function mirrorLine(lines: readonly string[], uid: string, stem: string, doneHeading: string): string | undefined {
	const task = parse(lines.join("\n"), { doneHeading }).tasks.find((t) => t.draft.uid === uid);
	if (task === undefined || task.draft.checked || task.draft.priority === undefined) return undefined;
	const draft = task.draft;
	const when = (w: When): string => (w.kind === "date" ? w.date : `${w.date} ${w.time}`);
	const parts = ["- [ ]"];
	if (draft.text !== "") parts.push(draft.text);
	parts.push(PRIORITY_EMOJI[task.draft.priority]);
	if (draft.recurrence !== undefined) parts.push(`🔁 ${draft.recurrence}`);
	if (draft.start !== undefined) parts.push(`🛫 ${when(draft.start)}`);
	if (draft.scheduled !== undefined) parts.push(`⏳ ${when(draft.scheduled)}`);
	if (draft.due !== undefined) parts.push(`📅 ${when(draft.due)}`);
	parts.push(sourceLink(stem, linkHeading(lines, task, doneHeading)));
	parts.push(`🆔 ${uid}`);
	return parts.join(" ");
}

/** The link a mirror line carries to its source note (§7): plain text when the stem cannot be in a wikilink. */
function sourceLink(stem: string, heading: string | undefined): string {
	const suffix = heading !== undefined ? `#${heading}` : "";
	return /[[\]|#]/.test(stem) ? `${stem}${suffix}` : `[[${stem}${suffix}|${stem}]]`;
}

function fmtWhen(w: When): string {
	return w.kind === "date" ? w.date : `${w.date} ${w.time}`;
}

/** A task line in canonical form (§6.1): text, then `<priority> 🔁 🛫 ⏳ 📅 ✅ ➕ 📁 🆔`. */
export function canonicalLine(indent: string, marker: string, draft: TaskDraft): string {
	const parts = [`${indent}${marker} [${draft.checked ? "x" : " "}]`];
	if (draft.text !== "") parts.push(draft.text);
	if (draft.priority !== undefined) parts.push(PRIORITY_EMOJI[draft.priority]);
	if (draft.recurrence !== undefined) parts.push(`🔁 ${draft.recurrence}`);
	if (draft.start !== undefined) parts.push(`🛫 ${fmtWhen(draft.start)}`);
	if (draft.scheduled !== undefined) parts.push(`⏳ ${fmtWhen(draft.scheduled)}`);
	if (draft.due !== undefined) parts.push(`📅 ${fmtWhen(draft.due)}`);
	if (draft.completedOn !== undefined) parts.push(`✅ ${draft.completedOn}`);
	if (draft.created !== undefined) parts.push(`➕ ${draft.created}`);
	// A request the daemon has not answered yet stays on the line.
	else if (draft.wantsCreated) parts.push("➕");
	if (draft.list !== undefined) parts.push(`📁 ${draft.list}`);
	if (draft.uid !== undefined) parts.push(`🆔 ${draft.uid}`);
	return parts.join(" ");
}

const same = (a: unknown, b: unknown): boolean => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/**
 * A note with what the user changed on the mirror line of its task `uid` (§7.1, the
 * engine's `mirror_edits`): `was` is the mirror line before the edit, `now` after it. A
 * field — text, priority, repeat rule, start, scheduled, due — is taken over only when
 * the user changed it and the note still shows the value the mirror line had; where the
 * note changed too, the note wins. The line is rewritten in canonical form. `"absent"`
 * when the note has no such task, undefined when nothing is to be taken over. The
 * checkbox is not handled here ({@link completedByUid}).
 */
export function mirrorEdited(
	lines: readonly string[],
	uid: string,
	was: string,
	now: string,
	stem: string,
	doneHeading: string,
): string[] | "absent" | undefined {
	const task: ParsedTask | undefined = parse(lines.join("\n"), { doneHeading }).tasks.find((t) => t.draft.uid === uid);
	if (task === undefined) return "absent";
	const before = parseLine(was)?.draft;
	const after = parseLine(now)?.draft;
	if (before === undefined || after === undefined || before.uid !== uid || after.uid !== uid) return undefined;
	const draft: TaskDraft = { ...task.draft };
	const link = sourceLink(stem, linkHeading(lines, task, doneHeading));
	const unlinked = (text: string): string | undefined => (text.endsWith(link) ? text.slice(0, -link.length).trimEnd() : undefined);
	const textWas = unlinked(before.text);
	const textNow = unlinked(after.text);
	if (textWas !== undefined && textNow !== undefined && textNow !== textWas && draft.text === textWas) draft.text = textNow;
	if (!same(after.priority, before.priority) && same(draft.priority, before.priority)) draft.priority = after.priority;
	if (!same(after.recurrence, before.recurrence) && same(draft.recurrence, before.recurrence)) draft.recurrence = after.recurrence;
	for (const field of ["due", "start", "scheduled"] as const) {
		if (!same(after[field], before[field]) && same(draft[field], before[field])) draft[field] = after[field];
	}
	if (same(draft, task.draft)) return undefined;
	const out = [...lines];
	out[task.lineNo - 1] = canonicalLine(task.raw.slice(0, task.indentChars), task.raw.trimStart()[0], draft);
	return out;
}

/**
 * The TODO.md view brought in line with one note task (§7): `line` is its mirror line,
 * or undefined when the view does not show the task. A missing line is added in its
 * priority's section, a line the view should not have is removed. A line that is there
 * but differs is rewritten (and moved when the priority changed) only when `rewrite`
 * says so — the caller then seals the view (§7.2): in an unsealed view a mirror line
 * that differs from the daemon's last render is an edit the daemon would apply to the
 * note. Undefined when the view stays as it is. `section`: the view of a root note
 * (§7.6), when `view` is not TODO.md but the lines of that note.
 */
export function mirrored(view: readonly string[], uid: string, line: string | undefined, rewrite = false, section?: View): string[] | undefined {
	if (section?.root === true) return within(view, section, (sub) => mirrored(sub, uid, line, rewrite, { ...section, root: false }));
	const layout = section?.layout ?? INBOX;
	const at = view.findIndex((l) => parseLine(l)?.draft.uid === uid);
	if (at >= 0 && !layout.mirror(view[at])) return undefined;
	if (line === undefined) return at >= 0 ? removed(view, at, section) : undefined;
	const priority = parseLine(line)?.draft.priority;
	if (priority === undefined) return undefined;
	if (at < 0) {
		const out = [...view];
		putIn(out, line, prioritySection(priority), layout);
		return out;
	}
	if (!rewrite || view[at] === line) return undefined;
	const out = [...view];
	out[at] = line;
	return refiledIn(out, at, false, layout) ?? out;
}

/** What a deleted mirror line comes to in the task's note (§7.1). */
export type Dropped =
	/** The note without the task's line, which was `line` on index `index`. */
	| { lines: string[]; index: number; line: string }
	/** The note changed the task since: it wins, and this is the mirror line the view shows again. */
	| { restore: string };

/**
 * The note of the task `uid` after the user deleted its mirror line `mirror` from the
 * view (§7.1): a mirror line is the task, so the task's line leaves the note — provided
 * the note still shows the task as the deleted line did (the engine's rule in
 * `mirror_edits`); otherwise the note wins and the line comes back. `stem` names the
 * note. Undefined when the note has no such task, or none the view would show.
 */
export function mirrorDropped(lines: readonly string[], uid: string, mirror: string, stem: string, doneHeading: string): Dropped | undefined {
	const index = taskIndex(lines, uid, doneHeading);
	const shown = index < 0 ? undefined : mirrorLine(lines, uid, stem, doneHeading);
	if (shown === undefined) return undefined;
	if (shown !== mirror) return { restore: shown };
	return { lines: lines.filter((_l, i) => i !== index), index, line: lines[index] };
}

/**
 * A note with the line of a task put back where {@link mirrorDropped} took it out — its
 * mirror line is in the view again (an undo, a paste). Undefined when the note has the
 * task `uid`.
 */
export function putBack(lines: readonly string[], uid: string, index: number, line: string): string[] | undefined {
	if (lines.some((l) => parseLine(l)?.draft.uid === uid)) return undefined;
	const out = [...lines];
	out.splice(Math.min(index, out.length), 0, line);
	return out;
}

/**
 * A note with its task `uid` completed (§7.1: the box of its mirror line was checked in
 * the view): `"absent"` when the note has no such task, undefined when the task is
 * completed already, else the note after {@link statusRepaired}.
 */
export function completedByUid(lines: readonly string[], uid: string, today: string, doneHeading: string): string[] | "absent" | undefined {
	const task = parse(lines.join("\n"), { doneHeading }).tasks.find((t) => t.draft.uid === uid);
	if (task === undefined) return "absent";
	if (task.draft.checked) return undefined;
	const checked = [...lines];
	checked[task.lineNo - 1] = flipCheck(lines[task.lineNo - 1], true);
	return statusRepaired(checked, task.lineNo - 1, today, doneHeading);
}

/** The source note a mirror line links to (§7): the `<stem>` of the wikilink right before its `🆔`. */
export function mirrorSource(line: string): string | undefined {
	return /\[\[([^\]|#]+)(?:#[^\]|]*)?\|[^\]]*\]\][ \t]*🆔/u.exec(line)?.[1];
}

/**
 * The task lines whose checkbox differs between two versions of a note that differ in
 * nothing else — what a tap on a checkbox in reading view changes. Empty when the
 * versions differ in any other way.
 */
export function toggledLines(before: readonly string[], after: readonly string[]): number[] {
	if (before.length !== after.length) return [];
	const toggled: number[] = [];
	for (let i = 0; i < after.length; i++) {
		if (before[i] === after[i]) continue;
		const at = checkOffset(after[i]);
		if (at === undefined || checkOffset(before[i]) !== at) return [];
		if (before[i].slice(0, at) !== after[i].slice(0, at) || before[i].slice(at + 1) !== after[i].slice(at + 1)) return [];
		toggled.push(i);
	}
	return toggled;
}

/** One block of lines replaced by another, in line indexes of the document before the change. */
export interface Splice {
	at: number;
	remove: number;
	insert: string[];
}

/**
 * The difference between two versions of a document as at most two splices, ordered and
 * disjoint: what {@link filed} does is change one line in place, or take lines out in
 * one place and put lines in at another. Lines between the two places are not part of
 * either splice, so a cursor resting there is not disturbed.
 */
export function splices(before: readonly string[], after: readonly string[]): Splice[] {
	let prefix = 0;
	while (prefix < before.length && prefix < after.length && before[prefix] === after[prefix]) prefix++;
	let suffix = 0;
	while (
		suffix < before.length - prefix &&
		suffix < after.length - prefix &&
		before[before.length - 1 - suffix] === after[after.length - 1 - suffix]
	) {
		suffix++;
	}
	const old = before.slice(prefix, before.length - suffix);
	const now = after.slice(prefix, after.length - suffix);
	if (old.length === 0 && now.length === 0) return [];

	const same = (a: readonly string[], b: readonly string[]): boolean => a.every((l, i) => l === b[i]);
	// Lines moved up: `now` = inserted + kept, `old` = kept + removed.
	let up = Math.min(old.length, now.length);
	while (up > 0 && !same(old.slice(0, up), now.slice(now.length - up))) up--;
	// Lines moved down: `old` = removed + kept, `now` = kept + inserted.
	let down = Math.min(old.length, now.length);
	while (down > 0 && !same(old.slice(old.length - down), now.slice(0, down))) down--;

	if (up === 0 && down === 0) return [{ at: prefix, remove: old.length, insert: now }];
	const pair: Splice[] =
		up >= down
			? [
					{ at: prefix, remove: 0, insert: now.slice(0, now.length - up) },
					{ at: prefix + up, remove: old.length - up, insert: [] },
				]
			: [
					{ at: prefix, remove: old.length - down, insert: [] },
					{ at: prefix + old.length, remove: 0, insert: now.slice(down) },
				];
	return pair.filter((s) => s.remove > 0 || s.insert.length > 0);
}
