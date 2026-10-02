-- Behaviour test for the token guard of the Neovim integration (§16); plain Lua:
--   lua neovim/test/guard_test.lua        (from the repository root)
package.path = "neovim/lua/?.lua;" .. package.path
local guard = require("restask.guard")

local A = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpa"
local B = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
local count = 0

local function eq(got, want, what)
	count = count + 1
	assert(got == want, ("%s: expected %q, got %q"):format(what, tostring(want), tostring(got)))
end

--- The text with `|` at offset `at`.
local function mark(text, at)
	return at and (text:sub(1, at) .. "|" .. text:sub(at + 1)) or text
end

-- §6.1: which lines have a token, and what is hidden.
local function shown(line)
	local token = guard.token(line)
	return token and (line:sub(1, token.start) .. line:sub(token.stop + 1)) or line
end
eq(shown("- [ ] Buy milk ➕ 2026-09-22" .. A), "- [ ] Buy milk ➕ 2026-09-22", "token at the end")
eq(shown("\t* [x] old 🆔 taskres-01jzq4tsvg2c9xkw7n5m8rhdpa tail"), "\t* [x] old tail", "legacy token, text behind")
eq(shown("- [ ]   🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpa"), "- [ ]   ", "token right after the checkbox")
eq(shown("- [ ] Buy milk  " .. A), "- [ ] Buy milk  ", "one blank is hidden, the others are text")
for _, line in ipairs({
	"prose" .. A,
	"1. [ ] ordered" .. A,
	"- [ ] no token",
	"- [ ] not a ULID 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpi",
	"- [ ] short 🆔 restask-01jzq4",
}) do
	eq(guard.token(line), nil, line)
end

-- Edits: `old` → `new` is what the editor did, `want` what the buffer should hold
-- (with the cursor); nil means the edit stands. Edits that keep the number of lines
-- never give a token up.
local repairs = {
	{ "typing in front of the token", "- [ ] a" .. A, "- [ ] a b" .. A, nil },
	{ "a blank typed at the visible end", "- [ ] a" .. A, "- [ ] a " .. A, nil },
	{ "typing behind the token", "- [ ] a" .. A, "- [ ] a" .. A .. " b", "- [ ] a b|" .. A },
	{ "D from the middle of the text", "- [ ] hello" .. A, "- [ ] he", "- [ ] he|" .. A },
	{ "C and typing", "- [ ] hello" .. A, "- [ ] hey", "- [ ] hey|" .. A },
	{ "x on a hidden character", "- [ ] a" .. A, "- [ ] a" .. A:sub(1, -2), "- [ ] a|" .. A },
	{ "substitute inside the token", "- [ ] a" .. A, "- [ ] a 🆔 foo-01jzq4tsvg2c9xkw7n5m8rhdpa", "- [ ] a|" .. A },
	{ "deleting the whole body", "- [ ] hello" .. A, "- [ ] ", "- [ ] |" .. A },
	{ "cc and retyping the line", "- [ ] hello" .. A, "- [ ] new", "- [ ] new|" .. A },
	{ "a rewrite that drops the token", "- [ ] hello" .. A, "* [x] other", "* [x] other|" .. A },
	{ "typing fast behind the token, then Enter", "- [ ] a" .. A, "- [ ] a" .. A .. "b\nc", "- [ ] ab" .. A .. "\nc|" },
	{ "joining lines from a line start", "x\n- [ ] hello" .. A .. "\ny", "xy", nil },
	{ "a change from the line above up to the token", "x\n- [ ] a" .. A, "y\nz" .. A, "y\nz" },
	{ "0D: the line emptied", "- [ ] hello" .. A, "", nil },
	{ "the text and the checkbox deleted", "- [ ] hello  " .. A, A, "" },
	{ "the checkbox deleted", "- [ ] hello" .. A, "- hello" .. A, "- hello" },
	{ "cc and retyping the line as prose", "- [ ] hello" .. A, "prose", nil },
	{ "a character that breaks the checkbox", "- [ ] a" .. A, "- [/] a" .. A, "- [/] a" },
	{ "J onto a line that is no task", "x\n- [ ] c" .. B, "x c" .. B, "x c" },
	{ "the line retyped as prose over the checkbox", "- [ ] a" .. A, "- [/] b", nil },
	{ "another bullet, still a task", "- [ ] a" .. A, "* [x] a" .. A, nil },
	{ "Enter at the visible end", "- [ ] a" .. A, "- [ ] a\n" .. A, "- [ ] a" .. A .. "\n |" },
	{ "Enter, Vim dropping the blank", "- [ ] a" .. A, "- [ ] a\n" .. A:sub(2), "- [ ] a" .. A .. "\n|" },
	{ "Enter with a list leader", "- [ ] a" .. A, "- [ ] a\n- " .. A:sub(2), "- [ ] a" .. A .. "\n- |" },
	{ "Enter continuing the checkbox", "- [ ] a" .. A, "- [ ] a\n- [ ] " .. A, "- [ ] a" .. A .. "\n- [ ]  |" },
	{ "Enter in the middle of the text", "- [ ] hello" .. A, "- [ ] he\nllo" .. A, "- [ ] he" .. A .. "\nllo" },
	{ "Enter in the middle, the checkbox continued", "- [ ] hello" .. A, "- [ ] he\n- [ ] llo" .. A, nil },
	{ "Enter behind the token", "- [ ] a" .. A, "- [ ] a" .. A .. "\n", nil },
	{ "joining the next line", "- [ ] a" .. A .. "\nmore", "- [ ] a" .. A .. " more", nil },
	{ "deleting across lines", "- [ ] hello" .. A .. "\n- [ ] world" .. B, "- [ ] herld" .. B, "- [ ] he|" .. A .. "rld" },
	{ "a change on both sides of the token", "- [ ] foo" .. A .. " foo", "- [ ] bar" .. A .. " bar", nil },
	{ "toggling the checkbox", "- [ ] a" .. A, "- [x] a ✅ 2026-10-01" .. A, nil },
	{ "a token on a line that is no task", "prose" .. A, "prose", nil },
	{ "multibyte text before the token", "- [ ] é😀" .. A, "- [ ] é", "- [ ] é|" .. A },
}
for _, case in ipairs(repairs) do
	local _, old_breaks = case[2]:gsub("\n", "")
	local _, new_breaks = case[3]:gsub("\n", "")
	local keep = old_breaks == new_breaks
	local fixed, caret = guard.repair(case[2], case[3], keep)
	eq(fixed and mark(fixed, caret), case[4], case[1])
	if fixed then
		eq(guard.repair(case[2], fixed, keep), nil, case[1] .. " (the repair is stable)")
	end
end

-- Cursor: {line, column, column it came from, insert mode} → column (nil: stays).
local line = "- [ ] ab" .. A
local tail = line .. " tail"
local s, e = 8, #line
local cursors = {
	{ "insert: behind the token", line, e, nil, true, s },
	{ "insert: inside the token", line, s + 3, nil, true, s },
	{ "insert: in front of the token", line, s, nil, true, nil },
	{ "insert: moving right at the line end", line, s + 1, s, true, s },
	{ "insert: behind a blank just typed", "- [ ] ab " .. A, s + 1, s, true, nil },
	{ "normal: on a blank typed at the visible end", "- [ ] ab " .. A, s, nil, false, nil },
	{ "insert: moving right over the token", tail, s + 1, s, true, e + 1 },
	{ "insert: text behind the token", tail, e + 2, nil, true, nil },
	{ "normal: on a hidden character", line, e - 1, nil, false, s - 1 },
	{ "normal: on the last visible character", line, s - 1, nil, false, nil },
	{ "normal: moving right at the line end", line, s, s - 1, false, s - 1 },
	{ "normal: moving right over the token", tail, s, s - 1, false, e },
	{ "normal: no token", "- [ ] plain", 10, nil, false, nil },
}
for _, case in ipairs(cursors) do
	eq(guard.settle(case[2], case[3], case[4], case[5]), case[6], case[1])
end

print(("ok - %d checks"):format(count))
