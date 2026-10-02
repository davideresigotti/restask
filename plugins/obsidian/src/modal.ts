/**
 * Pure autocomplete core (docs/spec/integrations.md §15.2): the keyword table, the
 * case-insensitive prefix filter, and the rule for when typing triggers suggestions.
 *
 * No Obsidian API, no I/O, no wall clock: the local date is injected so
 * `today`/`tomorrow` entries stay pure and testable. The Obsidian suggest classes
 * consuming this live in main.ts.
 */

/** One autocomplete entry: what the user types and what gets inserted (§15.2 table). */
export interface Suggestion {
	/** Keyword matched by case-insensitive `startsWith` against the typed fragment. */
	keyword: string;
	/** Text inserted into the task line when chosen. */
	insert: string;
}

const PRIORITY_SUGGESTIONS: readonly Suggestion[] = [
	{ keyword: "highest", insert: "🔺" },
	{ keyword: "high", insert: "⏫" },
	{ keyword: "medium", insert: "🔼" },
	{ keyword: "low", insert: "🔽" },
	{ keyword: "lowest", insert: "⏬" },
];

const DATE_FIELD_SUGGESTIONS: readonly Suggestion[] = [
	{ keyword: "due", insert: "📅 " },
	{ keyword: "start", insert: "🛫 " },
	{ keyword: "scheduled", insert: "⏳ " },
	{ keyword: "repeat", insert: "🔁 every " },
	// Bare: the date is written when the line is settled — today for a new task, the
	// server's creation date for one that exists (§6.4).
	{ keyword: "created", insert: "➕" },
];

/** Suggestion threshold: nothing matches until ≥ 2 letters are typed (§15.2). */
export const MIN_FRAGMENT_LENGTH = 2;

/** Adds `days` to a `YYYY-MM-DD` date (pure calendar arithmetic, leap-year safe). */
export function addDays(date: string, days: number): string {
	const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
	if (m === null) return date;
	const shifted = new Date(Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]) + days));
	const y = String(shifted.getUTCFullYear()).padStart(4, "0");
	const mo = String(shifted.getUTCMonth() + 1).padStart(2, "0");
	const d = String(shifted.getUTCDate()).padStart(2, "0");
	return `${y}-${mo}-${d}`;
}

/**
 * Keyword suggestions for the typed fragment: an entry matches iff its keyword
 * `startsWith` the fragment (case-insensitive). Fragments shorter than
 * {@link MIN_FRAGMENT_LENGTH} and zero-match results yield an empty list (menu hides).
 */
export function suggestionsFor(fragment: string, today: string): Suggestion[] {
	const query = fragment.toLowerCase();
	if (query.length < MIN_FRAGMENT_LENGTH) return [];
	const dated: readonly Suggestion[] = [
		{ keyword: "today", insert: `📅 ${today}` },
		{ keyword: "tomorrow", insert: `📅 ${addDays(today, 1)}` },
	];
	return [...PRIORITY_SUGGESTIONS, ...DATE_FIELD_SUGGESTIONS, ...dated].filter((s) =>
		s.keyword.startsWith(query),
	);
}

/** Where a keyword fragment being typed starts, and the fragment itself. */
export interface Trigger {
	/** 0-based column of the fragment's first letter. */
	start: number;
	/** The letters typed so far. */
	query: string;
}

// A task line up to the cursor: checkbox, then a body ending in a whole word of letters.
const TYPING_RE = /^[ \t]*[-*+][ \t]+\[[ xX]\][ \t]+(?:.*[ \t])?([A-Za-z]+)$/;

/**
 * Decides whether suggestions should open while typing (§15.2). `beforeCursor` is the
 * current line up to the cursor. Suggestions open only inside a task line, when the
 * cursor ends a whole word of at least {@link MIN_FRAGMENT_LENGTH} letters that is a
 * prefix of some keyword — so ordinary prose only triggers on words that could be one.
 */
export function triggerAt(beforeCursor: string, today: string): Trigger | undefined {
	const m = TYPING_RE.exec(beforeCursor);
	if (m === null) return undefined;
	const query = m[1];
	if (suggestionsFor(query, today).length === 0) return undefined;
	return { start: beforeCursor.length - query.length, query };
}
