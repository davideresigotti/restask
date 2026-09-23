/**
 * TypeScript port of the VTODO serializer (docs/spec/vtodo.md §8.1) for the exact
 * subset the plugin produces (complete/uncomplete, priority) — the cache writer for
 * `.taskres/tasks/<uid>.ics` (§15.3).
 *
 * Emits the same bytes as the Rust codec for the same task: CRLF endings, the §8.1
 * property order, §4 timestamp forms, iCalendar TEXT escaping, and 75-octet folding.
 * No Obsidian API and no wall clock: the filesystem is an injected AdapterPort and
 * the UTC instant is a parameter (AGENTS.md §3).
 */

import type { Priority, When } from "./markdown";

/** VTODO `PRIORITY` values per §3.2. */
const PRIORITY_ICAL: Readonly<Record<Priority, number>> = {
	highest: 1,
	high: 3,
	medium: 5,
	low: 7,
	lowest: 9,
};

/** `PRODID` of every emitted calendar (§8.1, golden contract). */
export const PRODID = "-//taskres//restask 0.1.0//EN";

/** Per-vault state directory (§15.3). */
export const CACHE_DIR = ".taskres";

/** Cache directory inside the vault (§15.3). */
export const TASKS_DIR = ".taskres/tasks";

/** The task fields the plugin can produce when mirroring a line into the cache. */
export interface CacheTask {
	uid: string;
	text: string;
	done: boolean;
	/** ✅ date (`YYYY-MM-DD`); emits `COMPLETED` when present (§4 midnight UTC). */
	completedOn: string | undefined;
	priority: Priority | undefined;
	due: When | undefined;
	start: When | undefined;
	scheduled: When | undefined;
	/** ➕ date (`YYYY-MM-DD`); midnight-UTC `CREATED`, `nowUtc` when absent (§8.1). */
	created: string | undefined;
	/** Nearest ancestor UID; emits `RELATED-TO;TOREL=PARENT` when present (§6.2). */
	parent: string | undefined;
	/** Vault-relative source path for `X-TASKRES-SOURCE` (§8.1). */
	sourcePath: string;
}

/** Minimal filesystem surface of Obsidian's `DataAdapter` (structurally satisfied by `app.vault.adapter`). */
export interface AdapterPort {
	exists(path: string): Promise<boolean>;
	mkdir(path: string): Promise<void>;
	write(path: string, data: string): Promise<void>;
}

const p2 = (n: number): string => String(n).padStart(2, "0");

/** Formats a Date as an iCalendar UTC instant `YYYYMMDDTHHMMSSZ` (§4). */
export function formatUtc(date: Date): string {
	return (
		`${date.getUTCFullYear()}${p2(date.getUTCMonth() + 1)}${p2(date.getUTCDate())}` +
		`T${p2(date.getUTCHours())}${p2(date.getUTCMinutes())}${p2(date.getUTCSeconds())}Z`
	);
}

/** iCalendar TEXT escaping (§8.1): `\` → `\\`, `;` → `\;`, `,` → `\,`, newline → `\n`. */
function escapeText(s: string): string {
	return s.replace(/\\/g, "\\\\").replace(/;/g, "\\;").replace(/,/g, "\\,").replace(/\r?\n/g, "\\n");
}

function basicDate(ymd: string): string {
	return ymd.replaceAll("-", "");
}

/** `YYYY-MM-DD` → `;VALUE=DATE:YYYYMMDD`; `… HH:MM` → `:YYYYMMDDTHHMM00` floating (§4). */
function icalWhen(value: When): { params: string; value: string } {
	if (value.kind === "date") return { params: ";VALUE=DATE", value: basicDate(value.date) };
	const [h, m] = value.time.split(":");
	return { params: "", value: `${basicDate(value.date)}T${h}${m}00` };
}

const encoder = new TextEncoder();
const byteLen = (s: string): number => encoder.encode(s).length;

/** RFC 5545 folding: physical lines ≤ 75 octets, continuations start with one SPACE, never splitting a codepoint. */
function fold(line: string): string {
	if (byteLen(line) <= 75) return line;
	const parts: string[] = [];
	let rest = line;
	let first = true;
	while (byteLen(rest) > (first ? 75 : 74)) {
		const limit = first ? 75 : 74;
		let cut = 0;
		let bytes = 0;
		for (const ch of rest) {
			const b = byteLen(ch);
			if (bytes + b > limit) break;
			bytes += b;
			cut += ch.length;
		}
		parts.push(first ? rest.slice(0, cut) : ` ${rest.slice(0, cut)}`);
		rest = rest.slice(cut);
		first = false;
	}
	parts.push(first ? rest : ` ${rest}`);
	return parts.join("\r\n");
}

/** Serializes a task to a full VCALENDAR (§8.1 property order, §4 timestamps, deterministic). */
export function toVcalendar(task: CacheTask, nowUtc: string): string {
	const props: string[] = [
		"BEGIN:VCALENDAR",
		"VERSION:2.0",
		`PRODID:${PRODID}`,
		"BEGIN:VTODO",
		`UID:${task.uid}`,
		`DTSTAMP:${nowUtc}`,
		`CREATED:${task.created !== undefined ? `${basicDate(task.created)}T000000Z` : nowUtc}`,
		`LAST-MODIFIED:${nowUtc}`,
		`SUMMARY:${escapeText(task.text)}`,
		`STATUS:${task.done ? "COMPLETED" : "NEEDS-ACTION"}`,
		`PERCENT-COMPLETE:${task.done ? "100" : "0"}`,
	];
	if (task.priority !== undefined) props.push(`PRIORITY:${PRIORITY_ICAL[task.priority]}`);
	if (task.start !== undefined) {
		const w = icalWhen(task.start);
		props.push(`DTSTART${w.params}:${w.value}`);
	}
	if (task.due !== undefined) {
		const w = icalWhen(task.due);
		props.push(`DUE${w.params}:${w.value}`);
	}
	if (task.completedOn !== undefined) props.push(`COMPLETED:${basicDate(task.completedOn)}T000000Z`);
	if (task.parent !== undefined) props.push(`RELATED-TO;TOREL=PARENT:${task.parent}`);
	if (task.scheduled !== undefined) {
		const w = icalWhen(task.scheduled);
		props.push(`X-TASKRES-SCHEDULED${w.params}:${w.value}`);
	}
	props.push(`X-TASKRES-SOURCE;VALUE=TEXT:${escapeText(task.sourcePath)}`);
	props.push("END:VTODO", "END:VCALENDAR");
	return props.map(fold).join("\r\n") + "\r\n";
}

/** Cache path for a UID (§15.3): `.taskres/tasks/<uid>.ics`. */
export function cachePath(uid: string): string {
	return `${TASKS_DIR}/${uid}.ics`;
}

/**
 * Writes the cache VTODO via the injected adapter (§15.3): creates `.taskres/tasks/`
 * as needed; when `.taskres/` is absent (fresh mobile device) it skips silently and
 * returns false, otherwise true after writing.
 */
export async function writeCacheFile(adapter: AdapterPort, task: CacheTask, nowUtc: string): Promise<boolean> {
	if (!(await adapter.exists(CACHE_DIR))) return false;
	if (!(await adapter.exists(TASKS_DIR))) await adapter.mkdir(TASKS_DIR);
	await adapter.write(cachePath(task.uid), toVcalendar(task, nowUtc));
	return true;
}
