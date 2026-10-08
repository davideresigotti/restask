/**
 * Hiding the `🆔` token in the editor (docs/spec/integrations.md §15.5).
 *
 * The token stays in the document; a decoration keeps it off the screen in Live Preview
 * and in source mode. A token nobody sees is easy to destroy, so the same extension makes
 * the hidden range behave like the end of the line it usually is: the cursor never rests
 * behind or inside it, and an edit cannot delete it or push it onto another line unless
 * it takes the task's whole body with it. An edit after which the token would show — its
 * line is no task any more — takes the token along.
 *
 * It also holds the editor side of §15.6: a task line the user has edited is settled
 * (`filing.ts` decides how) as soon as the cursor leaves it, or its checkbox is tapped.
 * And that of §15.7: a new line in a TODO section starts with a checkbox.
 *
 * This file is the only one that uses CodeMirror, which Obsidian provides at run time.
 */

import {
	Annotation,
	ChangeSet,
	EditorSelection,
	EditorState,
	MapMode,
	Prec,
	Text,
	Transaction,
	findClusterBreak,
	type Extension,
	type Range,
	type TransactionSpec,
} from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { sealLine, uidToken } from "./conceal";
import {
	TASK_START,
	declaresRoot,
	disclaimed,
	inTodoSection,
	inView,
	isMirrorShaped,
	isSealed,
	mirrorSource,
	mirrored,
	removed,
	resealed,
	routesItself,
	settled,
	splices,
	viewOf,
	type NoteKind,
	type Splice,
	type View,
} from "./filing";
import { bodyStart, checkOffset, parseLine } from "./markdown";

/** Marks the plugin's own registering and filing of a task (§15.6). */
const filing = Annotation.define<boolean>();

/** A hidden token in document offsets, with the line it sits on. */
interface DocToken {
	start: number;
	end: number;
	bodyStart: number;
	lineFrom: number;
	lineTo: number;
	/** First position right of the token: one character on, or the start of the next line. */
	past: number;
	/** Start of the character left of the token. */
	before: number;
}

function tokenOn(doc: Text, pos: number): DocToken | undefined {
	const line = doc.lineAt(pos);
	const token = uidToken(line.text);
	if (token === undefined) return undefined;
	const end = line.from + token.end;
	return {
		start: line.from + token.start,
		end,
		bodyStart: line.from + token.bodyStart,
		lineFrom: line.from,
		lineTo: line.to,
		past: end < line.to ? line.from + findClusterBreak(line.text, token.end, true) : Math.min(end + 1, doc.length),
		before: line.from + findClusterBreak(line.text, token.start, false),
	};
}

interface Edit {
	from: number;
	to: number;
	insert: Text;
}

/** Where the cursor goes after a change: behind an edit, or at a position of the old document. */
type Caret = { edit: Edit } | { pos: number; assoc: number };

/** Tokens a deletion of `from..to` would damage: touched, but not removed with their whole body. */
function damagedTokens(doc: Text, from: number, to: number): DocToken[] {
	const first = tokenOn(doc, from);
	const last = doc.lineAt(from).number === doc.lineAt(to).number ? undefined : tokenOn(doc, to);
	const damaged: DocToken[] = [];
	for (const token of [first, last]) {
		if (token === undefined || from >= token.end || to <= token.start) continue;
		if (from <= token.bodyStart && to >= token.end) continue;
		damaged.push(token);
	}
	return damaged;
}

/**
 * One change, rewritten so that no token is damaged:
 *
 * - a deletion or replacement is cut around the tokens it would damage; when nothing is
 *   left of a Backspace or Delete, it takes the visible character next to the token;
 * - text typed inside or right behind a token goes in front of it;
 * - a line break at the token goes behind it, so the token stays on its own line and the
 *   new line starts without one.
 */
function guardChange(
	doc: Text,
	from: number,
	to: number,
	insert: Text,
	direction: number,
): { edits: Edit[]; caret: Caret; altered: boolean } {
	const whole: Edit = { from, to, insert };
	let edits: Edit[] = [whole];
	let caret: Caret = { edit: whole };
	let altered = false;

	if (from === to) {
		const token = tokenOn(doc, from);
		if (token !== undefined && insert.lines === 1 && from > token.start && from <= token.end) {
			whole.from = token.start;
			whole.to = token.start;
			altered = true;
		}
	} else {
		const damaged = damagedTokens(doc, from, to);
		if (damaged.length > 0) {
			altered = true;
			edits = [];
			let pos = from;
			for (const token of damaged) {
				if (token.start > pos) edits.push({ from: pos, to: token.start, insert: Text.empty });
				pos = Math.max(pos, token.end);
			}
			if (to > pos) edits.push({ from: pos, to, insert: Text.empty });
			const first = damaged[0];
			if (insert.length > 0) {
				const at = Math.min(from, first.start);
				if (edits.length > 0 && edits[0].from === at) edits[0].insert = insert;
				else edits.unshift({ from: at, to: at, insert });
				caret = { edit: edits[0] };
			} else if (edits.length > 0) {
				caret = { edit: edits[0] };
			} else if (direction < 0 && first.start > first.bodyStart) {
				edits.push({ from: first.before, to: first.start, insert: Text.empty });
				caret = { edit: edits[0] };
			} else {
				if (direction > 0 && first.end < first.lineTo) {
					edits.push({ from: first.end, to: first.past, insert: Text.empty });
				}
				caret = { pos: first.start, assoc: -1 };
			}
		}
	}

	const out: Edit[] = [];
	for (const edit of edits) {
		// Only a break put into the token's own line can carry the token away.
		const token = edit.insert.lines > 1 ? tokenOn(doc, edit.to) : undefined;
		if (token === undefined || edit.from < token.lineFrom || edit.to < token.start || edit.to >= token.end) {
			out.push(edit);
			continue;
		}
		altered = true;
		const cut = edit.insert.line(1).to;
		const head: Edit = {
			from: Math.min(edit.from, token.start),
			to: Math.min(edit.to, token.start),
			insert: edit.insert.slice(0, cut),
		};
		const tail: Edit = { from: token.end, to: token.end, insert: edit.insert.slice(cut) };
		if (head.from < head.to || head.insert.length > 0) out.push(head);
		out.push(tail);
		if ("edit" in caret && caret.edit === edit) caret = { edit: tail };
	}
	// A moved line break may meet the rest of a cut deletion at the token's end: one edit.
	for (let i = 0; i + 1 < out.length; i++) {
		const a = out[i];
		const b = out[i + 1];
		if (a.from !== a.to || b.from !== a.from || b.insert.length > 0) continue;
		b.insert = a.insert;
		if ("edit" in caret && caret.edit === a) caret = { edit: b };
		out.splice(i, 1);
		i--;
	}
	return { edits: out, caret, altered };
}

/**
 * Keeps the cursor out of the hidden range. A cursor that lands inside or right behind a
 * token moves in front of it — except when it came from there by moving right, which
 * carries it past the token. A selection made inside the line's text stops in front of
 * the token likewise, so what is copied or deleted is what was visibly selected.
 */
function settle(selection: EditorSelection, doc: Text, before: EditorSelection | undefined): EditorSelection {
	let moved = false;
	const comparable = before !== undefined && before.ranges.length === selection.ranges.length;
	const ranges = selection.ranges.map((range, i) => {
		const token = tokenOn(doc, range.head);
		if (token === undefined || range.head <= token.start || range.head > token.end) return range;
		const canPass = range.head < token.end && token.past > token.end;
		if (range.empty) {
			const from = comparable ? before.ranges[i] : undefined;
			const rightwards = from !== undefined && from.empty && from.head === token.start;
			moved = true;
			return EditorSelection.cursor(rightwards && canPass ? token.past : token.start);
		}
		if (range.anchor <= token.bodyStart || range.anchor > token.start) return range;
		moved = true;
		return EditorSelection.range(range.anchor, range.anchor === token.start && canPass ? token.past : token.start);
	});
	return moved ? EditorSelection.create(ranges, selection.mainIndex) : selection;
}

/** The changes of `tr` with every damaging one rewritten; `altered` says whether one was. */
function guardChanges(tr: Transaction): { edits: Edit[]; carets: Caret[]; altered: boolean } {
	const doc = tr.startState.doc;
	const direction = tr.isUserEvent("delete.backward") ? -1 : tr.isUserEvent("delete.forward") ? 1 : 0;
	const edits: Edit[] = [];
	const carets: Caret[] = [];
	let altered = false;
	tr.changes.iterChanges((from, to, _fromB, _toB, insert) => {
		const guarded = guardChange(doc, from, to, insert, direction);
		edits.push(...guarded.edits);
		carets.push(guarded.caret);
		altered = altered || guarded.altered;
	});
	return { edits, carets, altered };
}

/**
 * The tokens `changes` (the `edits`, as one change set) would bring out of hiding: still
 * in the document, but on a line that is no task any more, or behind another task's
 * token. Such a token is no identity — the engine reads the line as prose — and would
 * only show. Returned as the edits of the changed document that put them away: the token
 * goes back to the head of its task when a line break cut it off from it, and is deleted
 * otherwise.
 */
function exposedTokens(doc: Text, edits: readonly Edit[], changes: ChangeSet): Edit[] {
	const after = changes.apply(doc);
	const seen = new Set<number>();
	const exposed: Edit[] = [];
	for (const edit of edits) {
		for (const pos of [edit.from, edit.to]) {
			const token = tokenOn(doc, pos);
			if (token === undefined || seen.has(token.start)) continue;
			seen.add(token.start);
			const start = changes.mapPos(token.start, 1);
			const end = changes.mapPos(token.end, -1);
			// Gone with the edit already.
			if (end - start !== token.end - token.start) continue;
			const line = after.lineAt(end);
			const hidden = uidToken(line.text);
			if (hidden !== undefined && line.from + hidden.end === end) continue;
			exposed.push({ from: start, to: end, insert: Text.empty });
			const head = after.lineAt(changes.mapPos(token.lineFrom, -1));
			if (head.number < line.number && bodyStart(head.text) !== undefined && uidToken(head.text) === undefined) {
				exposed.push({ from: head.to, to: head.to, insert: Text.of([` ${doc.sliceString(token.start, token.end).trimStart()}`]) });
			}
		}
	}
	return exposed;
}

/** True when each cursor of `tr` sits right behind one of its changes, as after typing. */
function cursorsFollowChanges(tr: Transaction): boolean {
	const ends: number[] = [];
	tr.changes.iterChanges((_from, _to, _fromB, toB) => ends.push(toB));
	const ranges = tr.newSelection.ranges;
	return ranges.length === ends.length && ranges.every((range, i) => range.empty && range.head === ends[i]);
}

function guarded(tr: Transaction): TransactionSpec | undefined {
	// A reload from disk and the history replay documents that were already valid.
	if (!tr.docChanged || tr.isUserEvent("set") || tr.isUserEvent("undo") || tr.isUserEvent("redo")) return undefined;
	// Filing moves whole lines, tokens included.
	if (tr.annotation(filing) === true) return undefined;
	const result = guardChanges(tr);
	const state = tr.startState;
	let changes: ChangeSet = tr.changes;
	let selection = tr.newSelection;
	try {
		if (result.altered) {
			const guardedChanges = state.changes(result.edits);
			changes = guardedChanges;
			selection = cursorsFollowChanges(tr)
				? EditorSelection.create(
						result.carets.map((caret) =>
							EditorSelection.cursor(
								"edit" in caret ? guardedChanges.mapPos(caret.edit.to, 1) : guardedChanges.mapPos(caret.pos, caret.assoc),
							),
						),
						tr.newSelection.mainIndex,
					)
				: state.selection.map(guardedChanges);
		}
		const exposed = exposedTokens(state.doc, result.edits, changes);
		if (!result.altered && exposed.length === 0) return undefined;
		if (exposed.length > 0) {
			const drop = ChangeSet.of(exposed, changes.newLength);
			changes = changes.compose(drop);
			selection = selection.map(drop);
		}
	} catch {
		return undefined;
	}
	const annotations = [];
	const userEvent = tr.annotation(Transaction.userEvent);
	if (userEvent !== undefined) annotations.push(Transaction.userEvent.of(userEvent));
	const addToHistory = tr.annotation(Transaction.addToHistory);
	if (addToHistory !== undefined) annotations.push(Transaction.addToHistory.of(addToHistory));
	return {
		changes,
		selection: settle(selection, changes.apply(state.doc), undefined),
		effects: tr.effects,
		annotations,
		scrollIntoView: tr.scrollIntoView,
	};
}

/**
 * The transaction filter behind §15.5's editing rules. It needs no view, so it is tested
 * against a bare `EditorState`.
 */
export const uidGuard: Extension = EditorState.transactionFilter.of((tr) => {
	const spec = guarded(tr);
	if (spec !== undefined) return spec;
	const settled = settle(tr.newSelection, tr.newDoc, tr.docChanged ? undefined : tr.startState.selection);
	return settled === tr.newSelection ? tr : [tr, { selection: settled, sequential: true }];
});

const HIDDEN = Decoration.replace({});

function hiddenRanges(view: EditorView): DecorationSet {
	const ranges: Range<Decoration>[] = [];
	const doc = view.state.doc;
	let done = 0;
	for (const { from, to } of view.visibleRanges) {
		for (let pos = from; pos <= to; ) {
			const line = doc.lineAt(pos);
			pos = line.to + 1;
			if (line.number <= done) continue;
			done = line.number;
			const token = uidToken(line.text);
			if (token !== undefined) ranges.push(HIDDEN.range(line.from + token.start, line.from + token.end));
		}
	}
	return Decoration.set(ranges);
}

const SEAL_HIDDEN = Decoration.line({ class: "restask-seal-line" });

/** The line decoration that hides the seal line of the frontmatter (§7.2), when the document has one. */
function hiddenSeal(view: EditorView): DecorationSet {
	const doc = view.state.doc;
	const head: string[] = [];
	for (let n = 1; n <= Math.min(doc.lines, 64); n++) {
		const text = doc.line(n).text;
		head.push(text);
		if (n > 1 && text === "---") break;
	}
	const at = sealLine(head);
	return at < 0 ? Decoration.none : Decoration.set([SEAL_HIDDEN.range(doc.line(at + 1).from)]);
}

const sealConcealer = ViewPlugin.fromClass(
	class {
		decorations: DecorationSet;

		constructor(view: EditorView) {
			this.decorations = hiddenSeal(view);
		}

		update(update: ViewUpdate): void {
			if (update.docChanged) this.decorations = hiddenSeal(update.view);
		}
	},
	{ decorations: (plugin) => plugin.decorations },
);

const uidConcealer = ViewPlugin.fromClass(
	class {
		decorations: DecorationSet;

		constructor(view: EditorView) {
			this.decorations = hiddenRanges(view);
		}

		update(update: ViewUpdate): void {
			if (update.docChanged || update.viewportChanged) this.decorations = hiddenRanges(update.view);
		}
	},
	{ decorations: (plugin) => plugin.decorations },
);

/** The editor extension of §15.5: the token is hidden, and edits cannot lose it by accident. */
export function uidConcealment(): Extension {
	return [uidGuard, uidConcealer, sealConcealer];
}

/** How carrying a mirror line's edit to its note went: done, no note has the task, or not possible now. */
export type Carried = "carried" | "unclaimed" | "failed";

/**
 * How deleting a mirror line's task from its note went (§7.1): the task's line is gone
 * from the note; no note has a task the view shows under that UID; the note could not
 * be written now; or the note changed the task since and wins — `restore` is the mirror
 * line the view shows again.
 */
export type DroppedMirror = "deleted" | "unclaimed" | "failed" | { restore: string };

/** What the editor needs from the plugin to settle tasks (§15.6). */
export interface FilingHost {
	/** Whether this device settles tasks itself (the `settleTasks` setting). */
	enabled(): boolean;
	/** Whether a new line in a TODO section starts with a checkbox (the `startTasks` setting, §15.7). */
	startsTasks(): boolean;
	/**
	 * Whether the note in `state` is the vault's TODO.md view (§7): the file at the inbox
	 * path, whatever it contains. The view a root note holds (§7.6) is found in the note
	 * itself and is not asked for here.
	 */
	isInboxView(state: EditorState): boolean;
	/** Whether the note in `state` is routed by a `restask-list-root` of its folder or one above (§5.2). */
	inheritsList(state: EditorState): boolean;
	/** The vault's `done_heading`. */
	doneHeading(): string;
	/** Device-local calendar date `YYYY-MM-DD`. */
	today(): string;
	/** A fresh task UID (§3.1). */
	uid(): string;
	/**
	 * Carries what the user changed on a mirror line to the source note of the task `uid`
	 * (§7.1); `state` is the view — TODO.md, or a root note (§7.6) — `was` the line
	 * before the edit (undefined when unknown: only a checked box is carried then) and
	 * `now` the line after it.
	 */
	carryMirror(state: EditorState, uid: string, was: string | undefined, now: string): Promise<Carried>;
	/**
	 * Deletes the task `uid` from its source note: the user deleted its mirror line
	 * `line` from the view in `state` (§7.1).
	 */
	dropMirror(state: EditorState, uid: string, line: string): Promise<DroppedMirror>;
	/**
	 * Tells the plugin that the mirror line of the task `uid`, deleted from the view in
	 * `state` before, is in it again (an undo, a paste): a task {@link dropMirror} took
	 * out of its note goes back there.
	 */
	mirrorReturned(state: EditorState, uid: string): void;
	/** Tells the plugin that the task `uid` of the note in `state` was edited or deleted: the views that show it follow (§7, §7.6). */
	noteTaskSettled(state: EditorState, uid: string): void;
}

/** One splice of lines as a change of `doc`; the line breaks go with the lines. */
function spliceChange(doc: Text, splice: Splice): { from: number; to: number; insert: string } {
	if (splice.remove === 1 && splice.insert.length === 1) {
		// A line changed in place: only its tail is replaced, what precedes it is not touched.
		const line = doc.line(splice.at + 1);
		const text = splice.insert[0];
		let same = 0;
		while (same < line.length && same < text.length && line.text[same] === text[same]) same++;
		// Never between the two halves of one character.
		const before = line.text.charCodeAt(same - 1);
		if (same > 0 && before >= 0xd800 && before <= 0xdbff) same--;
		return { from: line.from + same, to: line.to, insert: text.slice(same) };
	}
	if (splice.at >= doc.lines) return { from: doc.length, to: doc.length, insert: `\n${splice.insert.join("\n")}` };
	const from = doc.line(splice.at + 1).from;
	if (splice.at + splice.remove < doc.lines) {
		return { from, to: doc.line(splice.at + splice.remove + 1).from, insert: splice.insert.map((l) => `${l}\n`).join("") };
	}
	if (splice.insert.length > 0) return { from, to: doc.length, insert: splice.insert.join("\n") };
	return { from: Math.max(0, from - 1), to: doc.length, insert: "" };
}

/** The transaction that turns the document of `state` into `after`, or undefined when they are equal. */
function rewrite(state: EditorState, after: readonly string[]): TransactionSpec | undefined {
	const changes = splices(state.doc.toJSON(), after).map((splice) => spliceChange(state.doc, splice));
	return changes.length === 0 ? undefined : { changes, annotations: filing.of(true) };
}

/** How the note in `state` takes part (invariant 4), or undefined when it is local-only. */
function routedKind(state: EditorState, lines: readonly string[], host: FilingHost): NoteKind | undefined {
	const inboxView = host.isInboxView(state);
	if (!inboxView && !routesItself(lines) && !host.inheritsList(state)) return undefined;
	return { inboxView, doneHeading: host.doneHeading(), root: !inboxView && declaresRoot(lines) };
}

/** How the note in `state` takes part (§15.6), or undefined when it does not, or not on this device. */
function noteKind(state: EditorState, lines: readonly string[], host: FilingHost): NoteKind | undefined {
	return host.enabled() ? routedKind(state, lines, host) : undefined;
}

/**
 * The line break of `tr` that opened the line the cursor is now on, when that is all the
 * transaction did there: the line is new and empty. Enter, Shift+Enter and Vim's `o` on
 * a line that is no list item end this way; Obsidian's own continuation of a list item
 * does not (the new line has its marker already), nor does text that merely ends in a
 * line break (a paste, a template). Returns the cursor position, or undefined.
 */
function openedLine(tr: Transaction): number | undefined {
	if (!tr.docChanged || tr.annotation(filing) === true) return undefined;
	for (const event of ["set", "undo", "redo", "input.paste", "input.drop"]) if (tr.isUserEvent(event)) return undefined;
	const selection = tr.newSelection;
	if (selection.ranges.length !== 1 || !selection.main.empty) return undefined;
	const line = tr.newDoc.lineAt(selection.main.head);
	if (line.length > 0 || line.number === 1) return undefined;
	let opened = false;
	tr.changes.iterChanges((fromA, toA, _fromB, toB, insert) => {
		if (toB !== line.from || insert.lines !== 2 || insert.line(2).length > 0) return;
		// Obsidian writes the character in front of the cursor again, together with the break.
		const kept = insert.line(1).text;
		if (kept === "" || tr.startState.doc.sliceString(fromA, toA) === kept) opened = true;
	});
	return opened ? line.from : undefined;
}

/**
 * The editor extension of §15.7: a line the user opens in the TODO section of a routed
 * note — or anywhere above `## Done` in the TODO.md view — starts with `- [ ] `, the
 * cursor behind it, in the same undo step as the line break. Only a new, empty line
 * gets one: a line Obsidian continued a list on has its own, and Enter on the empty
 * checkbox takes it away again (Obsidian's rule for an empty list item), which is how a
 * line that is no task is written there. It looks at the transaction after §15.5's guard
 * has (filters run from the lowest precedence up): a break made in front of a hidden
 * token opens its line behind it. It needs no view, so it is tested against a bare
 * `EditorState`.
 */
export function taskStart(host: FilingHost): Extension {
	const filter = EditorState.transactionFilter.of((tr) => {
		const at = openedLine(tr);
		if (at === undefined || !host.startsTasks()) return tr;
		const lines = tr.newDoc.toJSON();
		// The note is the one the transaction started in; its new state is not computed for the asking.
		const note = routedKind(tr.startState, lines, host);
		if (note === undefined || !inTodoSection(lines, tr.newDoc.lineAt(at).number - 1, note)) return tr;
		return [tr, { changes: { from: at, insert: TASK_START }, selection: EditorSelection.cursor(at + TASK_START.length), sequential: true }];
	});
	return Prec.highest(filter);
}

/** What settling one line of an editor comes to. */
export interface Filing {
	/** The edit to dispatch, when the document changes. */
	spec?: TransactionSpec;
	/** `true` in the TODO.md view. */
	inboxView: boolean;
	/** The UID of the task on the line. */
	uid?: string;
	/** `true` for a mirror line of a view — TODO.md, or the one a root note holds: its edits belong in the source note (§7.1). */
	mirror: boolean;
	/** The line as it is now, for a mirror line. */
	now?: string;
	/** For a mirror line moved to another section (§7.4): the line as it arrived, before it took the section's priority. */
	was?: string;
}

/**
 * What settles the task on the line at `pos` (§15.6), or undefined when there is nothing
 * to settle: the device leaves it to the daemon, the note is not routed, or the line is
 * no task with text. It needs no view, so it is tested against a bare `EditorState`.
 * `moved`: the line arrived where it is — a paste, a moved line — rather than being
 * edited in place (§7.4).
 */
export function filingOf(state: EditorState, pos: number, host: FilingHost, moved = false): Filing | undefined {
	const lines = state.doc.toJSON();
	const note = noteKind(state, lines, host);
	if (note === undefined) return undefined;
	const idx = state.doc.lineAt(pos).number - 1;
	const result = settled(lines, idx, note, host.today(), () => host.uid(), false, moved);
	if (result === undefined) return undefined;
	return {
		spec: result.lines === undefined ? undefined : rewrite(state, result.lines),
		inboxView: note.inboxView,
		uid: result.uid,
		mirror: result.mirror === true,
		now: result.mirror === true ? (result.moved?.now ?? lines[idx]) : undefined,
		was: result.moved?.was,
	};
}

/**
 * `true` when the registered task on `text` arrived on its line rather than being
 * edited there (§7.4): `was`, the line before the user's edit, did not hold it — a
 * pasted line, a line moved by a command, a line put on an empty one.
 */
export function arrived(text: string, was: string | undefined): boolean {
	const uid = parseLine(text)?.draft.uid;
	return uid !== undefined && (was === undefined || parseLine(was)?.draft.uid !== uid);
}

/**
 * The view's side of a mirror edit (§7.1), once the source note was dealt with. A
 * mirror line the next render would not show — its box is checked, or it has no
 * priority left — leaves the view when the note took the edit; a checked line that no
 * note has a task for is filed as the task of the view's own it then is. Undefined when
 * nothing is to be done: the edit did not go through (the line stays as the user left
 * it, for the daemon to carry), or the line is gone.
 */
export function carriedSpec(state: EditorState, uid: string, outcome: Carried, host: FilingHost): TransactionSpec | undefined {
	if (outcome === "failed") return undefined;
	const lines = state.doc.toJSON();
	const note = noteKind(state, lines, host);
	const view = note === undefined ? undefined : viewOf(lines, note);
	if (note === undefined || view === undefined) return undefined;
	const idx = lines.findIndex((line, i) => inView(view, i) && parseLine(line)?.draft.uid === uid);
	const draft = idx < 0 ? undefined : parseLine(lines[idx])?.draft;
	if (draft === undefined) return undefined;
	let after: string[] | undefined;
	if (outcome === "carried") {
		// Until the view is sealed again, a line the plugin took out must not read as the user's deletion (§7.1).
		if (draft.checked || draft.priority === undefined) after = disclaimed(removed(lines, idx, view));
	} else if (draft.checked) {
		after = settled(lines, idx, note, host.today(), () => host.uid(), true)?.lines;
	}
	return after === undefined ? undefined : rewrite(state, after);
}

/** The view with `line` — the mirror line of a task the note has changed since, deleted from the view — put back (§7.1), or undefined. */
export function restoredSpec(state: EditorState, uid: string, line: string, host: FilingHost): TransactionSpec | undefined {
	const lines = state.doc.toJSON();
	const view = viewIn(state, lines, host);
	if (view === undefined) return undefined;
	const after = mirrored(lines, uid, line, false, view);
	return after === undefined ? undefined : rewrite(state, after);
}

/** The view the note in `state` holds — TODO.md, or the TODO section of a root note (§7.6) — when this device settles tasks. */
function viewIn(state: EditorState, lines: readonly string[], host: FilingHost): View | undefined {
	const note = noteKind(state, lines, host);
	return note === undefined ? undefined : viewOf(lines, note);
}

/** `true` when the note in `state` holds a view that is exactly as some device rendered it (§7.2). */
export function sealedView(state: EditorState, host: FilingHost): boolean {
	const lines = state.doc.toJSON();
	const view = viewIn(state, lines, host);
	return view !== undefined && isSealed(lines, view);
}

/** The view sealed again (§7.2), or undefined when its seal is right or the note has none. */
export function sealSpec(state: EditorState, host: FilingHost): TransactionSpec | undefined {
	const lines = state.doc.toJSON();
	const view = viewIn(state, lines, host);
	return view === undefined ? undefined : rewrite(state, resealed(lines, view));
}

/** Lines one paste may hand to the filer; the daemon settles what is beyond. */
const MAX_TOUCHED = 200;

/** While an input method composes a word the document is left alone; looked at again after this many ms. */
const COMPOSING_RETRY_MS = 250;

/** `true` for an edit made by the user here and now — not a reload, not history, not the filer itself. */
function typed(tr: Transaction): boolean {
	return (
		tr.docChanged &&
		tr.annotation(filing) !== true &&
		!tr.isUserEvent("set") &&
		!tr.isUserEvent("undo") &&
		!tr.isUserEvent("redo")
	);
}

/** A line the user edited and that was not looked at since. */
interface Touched {
	/** A position in the line. */
	pos: number;
	/** The edit was the checkbox itself: the line is settled even with the cursor on it. */
	toggled: boolean;
	/** The line before the user's first edit of it; undefined when it did not exist as one line. */
	was: string | undefined;
}

/** A task whose line the user deleted. */
interface Gone {
	uid: string;
	/** The deleted line, to tell a mirror line and its note. */
	line: string;
}

const UID_TOKEN = /🆔[ \t]+((?:restask|taskres)-[0-9a-z]{26})/gu;

/**
 * The editor extension of §15.6. It remembers the lines the user edits and settles the
 * task on such a line once the cursor has left it or the editor lost the focus — at
 * once when the edit was the checkbox itself. A line still being typed in is never
 * touched, and only lines edited in this editor are: the rest of the note stays the
 * daemon's job. A mirror line deleted from a view — TODO.md, or the one a root note
 * holds (§7.6) — deletes the task in its note, and the task goes back there when the
 * line returns (§7.1). In a view it also keeps the seal (§7.2): a view that was sealed
 * when it was loaded is sealed again once every edit made in it has been settled and
 * has reached its note.
 */
export function taskFiling(host: FilingHost): Extension {
	return ViewPlugin.fromClass(
		class {
			private readonly view: EditorView;
			private touched: Touched[] = [];
			private gone: Gone[] = [];
			/** Tasks whose mirror line the user deleted here: back in their note when the line returns. */
			private readonly dropped = new Set<string>();
			private timer: ReturnType<typeof setTimeout> | undefined;
			private destroyed = false;
			/** The document was a sealed view when it was loaded, and no edit made in it since failed to reach its note. */
			private pure: boolean;
			/** Edits of other files that have not come back yet. */
			private pending = 0;

			constructor(view: EditorView) {
				this.view = view;
				this.pure = sealedView(view.state, host);
			}

			update(update: ViewUpdate): void {
				if (update.docChanged) {
					this.returned(update.state);
					const replaced = update.transactions.some((tr) => tr.isUserEvent("set"));
					if (replaced) {
						this.touched = [];
						this.gone = [];
					} else {
						this.touched = this.touched.flatMap((line) => {
							const pos = update.changes.mapPos(line.pos, 1, MapMode.TrackDel);
							return pos === null ? [] : [{ ...line, pos }];
						});
					}
					if (update.transactions.some(typed)) this.remember(update);
					// What is on disk, or what history brings back, is taken as it is.
					else if (!update.transactions.some((tr) => tr.annotation(filing) === true)) {
						this.pure = sealedView(update.state, host);
					}
				}
				const waiting = this.touched.length > 0 || this.gone.length > 0;
				if (waiting && (update.docChanged || update.selectionSet || update.focusChanged)) this.schedule(0);
			}

			destroy(): void {
				this.destroyed = true;
				if (this.timer !== undefined) clearTimeout(this.timer);
				this.timer = undefined;
			}

			/** Mirror lines deleted here that are in the view again — undone, or pasted elsewhere: their tasks go back to their notes. */
			private returned(state: EditorState): void {
				if (this.dropped.size === 0) return;
				const text = state.doc.toString();
				for (const uid of [...this.dropped]) {
					if (!text.includes(uid)) continue;
					this.dropped.delete(uid);
					host.mirrorReturned(state, uid);
				}
			}

			private remember(update: ViewUpdate): void {
				const before = update.startState.doc;
				const doc = update.state.doc;
				let text: string | undefined;
				update.changes.iterChanges((fromA, toA, fromB, toB) => {
					// A whole document put in place is not an edit of its lines (the editor's `setValue`).
					if (fromA === 0 && toA === before.length && toA > 0) {
						this.pure = false;
						return;
					}
					const old = before.lineAt(fromA);
					const first = doc.lineAt(fromB);
					const oneLine = old.to >= toA && first.to >= toB;
					const toggled = oneLine && toA - fromA === 1 && toB - fromB === 1 && checkOffset(first.text) === fromB - first.from;
					for (let line = first; this.touched.length < MAX_TOUCHED; line = doc.line(line.number + 1)) {
						const known = this.touched.find((t) => doc.lineAt(t.pos).from === line.from);
						if (known !== undefined) known.toggled = known.toggled || toggled;
						else this.touched.push({ pos: line.from, toggled, was: oneLine ? old.text : undefined });
						if (line.to >= toB || line.number === doc.lines) break;
					}
					// Tasks whose line is gone: their UID was in the lines the change covered and is nowhere now.
					if (fromA === toA) return;
					for (let line = old; this.gone.length < MAX_TOUCHED; line = before.line(line.number + 1)) {
						for (const match of line.text.matchAll(UID_TOKEN)) {
							text = text ?? doc.toString();
							if (!text.includes(match[1])) this.gone.push({ uid: match[1], line: line.text });
						}
						if (line.to >= toA || line.number === before.lines) break;
					}
				});
			}

			/** The work is done outside the update that asked for it: a view cannot be changed while it updates. */
			private schedule(delay: number): void {
				if (this.timer !== undefined || this.destroyed) return;
				this.timer = setTimeout(() => {
					this.timer = undefined;
					this.run();
				}, delay);
			}

			private run(): void {
				const view = this.view;
				if (view.composing) {
					this.schedule(COMPOSING_RETRY_MS);
					return;
				}
				this.settleGone();
				for (let round = 0; round <= MAX_TOUCHED; round++) {
					const state = view.state;
					const lineOf = (pos: number): number => state.doc.lineAt(Math.min(pos, state.doc.length)).from;
					const typing = view.hasFocus ? state.selection.ranges.map((range) => lineOf(range.head)) : [];
					const at = this.touched.findIndex((line) => line.toggled || !typing.includes(lineOf(line.pos)));
					if (at < 0) break;
					const [line] = this.touched.splice(at, 1);
					const pos = Math.min(line.pos, state.doc.length);
					const result = filingOf(state, pos, host, arrived(state.doc.lineAt(pos).text, line.was));
					if (result === undefined) continue;
					if (result.spec !== undefined) view.dispatch(result.spec);
					if (result.uid === undefined) continue;
					// A moved mirror line carries the priority it took from its section (§7.4).
					const was = result.was ?? line.was;
					if (result.mirror) {
						if (result.now !== undefined && result.now !== was) this.carry(result.uid, was, result.now);
					} else if (!result.inboxView) host.noteTaskSettled(view.state, result.uid);
				}
				this.seal();
			}

			/** Deleted lines: a note task takes its mirror line along; a deleted mirror line takes its task along (§7.1). */
			private settleGone(): void {
				const state = this.view.state;
				const lines = state.doc.toJSON();
				const note = noteKind(state, lines, host);
				const gone = this.gone;
				this.gone = [];
				if (note === undefined) return;
				for (const task of gone) {
					if (state.doc.toString().includes(task.uid)) continue;
					// In a note the line was the task — but for a mirror line in the view of a root note (§7.6).
					if (!note.inboxView && !(note.root === true && isMirrorShaped(task.line))) {
						host.noteTaskSettled(state, task.uid);
						continue;
					}
					if (mirrorSource(task.line) === undefined && !/\]\][ \t]*🆔/u.test(task.line)) continue;
					this.dropped.add(task.uid);
					this.await(host.dropMirror(state, task.uid, task.line), (outcome) => {
						// Not carried: the seal stays broken, and the daemon deletes the task by §7.1.
						if (outcome === "failed") this.pure = false;
						return typeof outcome === "string" ? undefined : restoredSpec(this.view.state, task.uid, outcome.restore, host);
					});
				}
			}

			private carry(uid: string, was: string | undefined, now: string): void {
				this.await(host.carryMirror(this.view.state, uid, was, now), (outcome) => {
					if (outcome === "failed" || (outcome === "unclaimed" && parseLine(now)?.draft.checked !== true)) this.pure = false;
					return carriedSpec(this.view.state, uid, outcome, host);
				});
			}

			/** Runs `then` on the result of an edit of another file, dispatches what it asks for, and seals when all are back. */
			private await<T>(job: Promise<T>, then: (result: T) => TransactionSpec | undefined): void {
				this.pending++;
				void job
					.then(
						(result) => {
							if (this.destroyed) return;
							const spec = then(result);
							if (spec !== undefined) this.view.dispatch(spec);
						},
						() => {
							this.pure = false;
						},
					)
					.finally(() => {
						this.pending--;
						if (!this.destroyed) this.seal();
					});
			}

			/** Seals the view again when it was sealed and nothing made in it is still on its way (§7.2). */
			private seal(): void {
				if (!this.pure || this.pending > 0 || this.touched.length > 0 || this.gone.length > 0) return;
				const spec = sealSpec(this.view.state, host);
				if (spec !== undefined) this.view.dispatch(spec);
			}
		},
	);
}
