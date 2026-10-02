-- Behaviour test for the pure part of the Neovim integration; runs under plain Lua:
--   lua neovim/test/toggle_test.lua        (from the repository root)
vim = { fn = {}, api = {}, cmd = function() end, log = { levels = {} }, keymap = {}, bo = {} }
package.path = "neovim/lua/?.lua;" .. package.path
local toggle = require("restask.toggle")

local cases = {
	{ "- [ ] buy milk", "done" },
	{ "  * [x] done one", "undone" },
	{ "+ [X] shout", "undone" },
	{ "- [ ] mentions [x] inside", "done" },
	{ "\t- [ ] tab indented", "done" },
	{ "plain [ ] text", nil },
	{ "1. [ ] ordered list", nil },
	{ "-[ ] no space after the marker", nil },
	{ "- [x]", nil },
	{ "# heading", nil },
}
for _, case in ipairs(cases) do
	local got = toggle.action_for(case[1])
	assert(got == case[2], ("%q: expected %s, got %s"):format(case[1], tostring(case[2]), tostring(got)))
end

local conceal = require("restask.conceal")
local modes = {
	{ "", "nc", "nc" },
	{ "c", "nc", "cn" },
	{ "nvic", "nc", "nvic" },
	{ "i", "", "i" },
}
for _, case in ipairs(modes) do
	local got = conceal.with_modes(case[1], case[2])
	assert(got == case[3], ("with_modes(%q, %q): expected %q, got %q"):format(case[1], case[2], case[3], got))
end
local settle = require("restask.settle")
local argv = settle.command("/vault/notes/a b.md")
assert(table.concat(argv, "|") == "restask|settle|--file|/vault/notes/a b.md", table.concat(argv, "|"))
local ran = false
settle.without(function()
	ran = true
end)
assert(ran, "without() runs its function")

print(("ok - %d cases"):format(#cases + #modes + 2))
