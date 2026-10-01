//! §8.1 serializer conformance: byte-parity with docs/contracts/vtodo-golden.ics,
//! §4 timestamp forms, escaping, folding, and the §15.3 cache-write behaviour.

import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
	cachePath,
	formatUtc,
	toVcalendar,
	writeCacheFile,
	type AdapterPort,
	type CacheTask,
} from "../src/vtodo";

const NOW = "20260922T143000Z";
const GOLDEN_UID = "restask-01jzetq1v2h3k4m5n6p7r8t9w0";

const GOLDEN_BYTES = readFileSync(
	fileURLToPath(new URL("../../../docs/contracts/vtodo-golden.ics", import.meta.url)),
);

function goldenTask(): CacheTask {
	return {
		uid: GOLDEN_UID,
		text: "Setup SSL certificate renew alert",
		done: false,
		completedOn: undefined,
		priority: "highest",
		due: { kind: "date", date: "2026-09-25" },
		start: undefined,
		scheduled: { kind: "date", date: "2026-09-23" },
		created: "2026-09-19",
		parent: undefined,
		sourcePath: "Home Lab Test.md",
	};
}

function memAdapter(initial: string[] = []) {
	const entries = new Set(initial);
	const contents = new Map<string, string>();
	const writes: string[] = [];
	const mkdirs: string[] = [];
	const adapter: AdapterPort & { writes: string[]; mkdirs: string[]; contents: Map<string, string> } = {
		exists: (p) => Promise.resolve(entries.has(p)),
		mkdir: (p) => {
			mkdirs.push(p);
			entries.add(p);
			return Promise.resolve();
		},
		write: (p, data) => {
			writes.push(p);
			entries.add(p);
			contents.set(p, data);
			return Promise.resolve();
		},
		writes,
		mkdirs,
		contents,
	};
	return adapter;
}

describe("§8.1 serializer", () => {
	it("golden byte parity", () => {
		const out = Buffer.from(toVcalendar(goldenTask(), NOW), "utf-8");
		expect(out.equals(GOLDEN_BYTES)).toBe(true);
	});

	it("deterministic for identical inputs", () => {
		expect(toVcalendar(goldenTask(), NOW)).toBe(toVcalendar(goldenTask(), NOW));
	});

	it("complete and uncomplete", () => {
		const done = toVcalendar({ ...goldenTask(), done: true, completedOn: "2026-09-23" }, NOW);
		expect(done).toContain("STATUS:COMPLETED\r\n");
		expect(done).toContain("PERCENT-COMPLETE:100\r\n");
		expect(done).toContain("COMPLETED:20260923T000000Z\r\n");
		const active = toVcalendar(goldenTask(), NOW);
		expect(active).toContain("STATUS:NEEDS-ACTION\r\n");
		expect(active).toContain("PERCENT-COMPLETE:0\r\n");
		expect(active).not.toContain("COMPLETED:");
	});

	it("priority mapping per §3.2, omitted when none", () => {
		expect(toVcalendar({ ...goldenTask(), priority: "highest" }, NOW)).toContain("PRIORITY:1\r\n");
		expect(toVcalendar({ ...goldenTask(), priority: "high" }, NOW)).toContain("PRIORITY:3\r\n");
		expect(toVcalendar({ ...goldenTask(), priority: "medium" }, NOW)).toContain("PRIORITY:5\r\n");
		expect(toVcalendar({ ...goldenTask(), priority: "low" }, NOW)).toContain("PRIORITY:7\r\n");
		expect(toVcalendar({ ...goldenTask(), priority: "lowest" }, NOW)).toContain("PRIORITY:9\r\n");
		const none = toVcalendar({ ...goldenTask(), priority: undefined }, NOW);
		expect(none).not.toContain("PRIORITY:");
	});

	it("date vs floating datetime due", () => {
		const dateOnly = toVcalendar({ ...goldenTask(), due: { kind: "date", date: "2026-09-19" } }, NOW);
		expect(dateOnly).toContain("DUE;VALUE=DATE:20260919\r\n");
		const timed = toVcalendar(
			{ ...goldenTask(), due: { kind: "datetime", date: "2026-09-19", time: "17:00" } },
			NOW,
		);
		expect(timed).toContain("DUE:20260919T170000\r\n");
	});

	it("dtstart and scheduled forms", () => {
		const out = toVcalendar(
			{
				...goldenTask(),
				start: { kind: "date", date: "2026-09-20" },
				scheduled: { kind: "datetime", date: "2026-09-21", time: "08:30" },
			},
			NOW,
		);
		expect(out).toContain("DTSTART;VALUE=DATE:20260920\r\n");
		expect(out).toContain("X-RESTASK-SCHEDULED:20260921T083000\r\n");
	});

	it("created defaults to now when absent", () => {
		const out = toVcalendar({ ...goldenTask(), created: undefined }, NOW);
		expect(out).toContain(`CREATED:${NOW}\r\n`);
	});

	it("related-to parent", () => {
		const out = toVcalendar({ ...goldenTask(), parent: "restask-01jzq4tsvg2c9xkw7n5m8rhdpb" }, NOW);
		expect(out).toContain("RELATED-TO;TOREL=PARENT:restask-01jzq4tsvg2c9xkw7n5m8rhdpb\r\n");
	});

	it("summary escaping", () => {
		const out = toVcalendar({ ...goldenTask(), text: "a;b,c\\d\ne" }, NOW);
		expect(out).toContain("SUMMARY:a\\;b\\,c\\\\d\\ne\r\n");
	});

	it("crlf endings everywhere", () => {
		const out = toVcalendar(goldenTask(), NOW);
		expect(out.endsWith("\r\n")).toBe(true);
		expect(out.replace(/\r\n/g, "")).not.toContain("\n");
		expect(out.replace(/\r\n/g, "")).not.toContain("\r");
	});
});

describe("§8.1 folding", () => {
	it("long summary with emoji folds to ≤75 octets and unfolds losslessly", () => {
		const long = "Skill issues with ".repeat(12) + "🔺⏫🔼🔽⏬" + " end";
		const out = toVcalendar({ ...goldenTask(), text: long }, NOW);
		for (const line of out.split("\r\n")) {
			if (line.length === 0) continue;
			expect(Buffer.byteLength(line, "utf-8")).toBeLessThanOrEqual(75);
		}
		expect(out.replace(/\r\n /g, "")).toContain(`SUMMARY:${long}`);
	});
});

describe("§15.3 cache writer", () => {
	it("skips silently when .restask is absent", async () => {
		const adapter = memAdapter();
		await expect(writeCacheFile(adapter, goldenTask(), NOW)).resolves.toBe(false);
		expect(adapter.writes).toEqual([]);
		expect(adapter.mkdirs).toEqual([]);
	});

	it("creates .restask/tasks as needed and writes the vtodo", async () => {
		const adapter = memAdapter([".restask"]);
		await expect(writeCacheFile(adapter, goldenTask(), NOW)).resolves.toBe(true);
		expect(adapter.mkdirs).toEqual([".restask/tasks"]);
		expect(adapter.writes).toEqual([cachePath(GOLDEN_UID)]);
		expect(adapter.contents.get(cachePath(GOLDEN_UID))).toBe(toVcalendar(goldenTask(), NOW));
	});

	it("writes directly when .restask/tasks exists", async () => {
		const adapter = memAdapter([".restask", ".restask/tasks"]);
		await expect(writeCacheFile(adapter, goldenTask(), NOW)).resolves.toBe(true);
		expect(adapter.mkdirs).toEqual([]);
		expect(adapter.writes).toEqual([cachePath(GOLDEN_UID)]);
	});

	it("formatUtc renders basic UTC instants", () => {
		expect(formatUtc(new Date(Date.UTC(2026, 8, 22, 14, 30, 0)))).toBe("20260922T143000Z");
		expect(formatUtc(new Date(Date.UTC(2026, 0, 5, 3, 7, 9)))).toBe("20260105T030709Z");
	});
});
