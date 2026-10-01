/**
 * Hiding the `🆔` token in the editor (docs/spec/integrations.md §15.5).
 *
 * The token stays in the document; a decoration keeps it off the screen in Live Preview
 * and in source mode. A token nobody sees is easy to destroy, so the same extension makes
 * the hidden range behave like the end of the line it usually is: the cursor never rests
 * behind or inside it, and an edit cannot delete it or push it onto another line unless
 * it takes the task's whole body with it.
 *
 * This file is the only one that uses CodeMirror, which Obsidian provides at run time.
 */

import {
	EditorSelection,
	EditorState,
	Text,
	Transaction,
	findClusterBreak,
	type Extension,
	type Range,
	type TransactionSpec,
} from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { uidToken } from "./conceal";

/** A hidden token in document offsets, with the line it sits on. */
interface DocToken {
	start: number;
	end: number;
	bodyStart: number;
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
		const token = edit.insert.lines > 1 ? tokenOn(doc, edit.to) : undefined;
		if (token === undefined || edit.to < token.start || edit.to >= token.end) {
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

/** The changes of `tr` with every damaging one rewritten, or undefined when none was. */
function guardChanges(tr: Transaction): { edits: Edit[]; carets: Caret[] } | undefined {
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
	return altered ? { edits, carets } : undefined;
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
	const result = guardChanges(tr);
	if (result === undefined) return undefined;
	const state = tr.startState;
	let changes;
	try {
		changes = state.changes(result.edits);
	} catch {
		return undefined;
	}
	const selection = cursorsFollowChanges(tr)
		? EditorSelection.create(
				result.carets.map((caret) =>
					EditorSelection.cursor(
						"edit" in caret ? changes.mapPos(caret.edit.to, 1) : changes.mapPos(caret.pos, caret.assoc),
					),
				),
				tr.newSelection.mainIndex,
			)
		: state.selection.map(changes);
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
	return [uidGuard, uidConcealer];
}
