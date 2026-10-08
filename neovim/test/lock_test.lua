-- Behaviour test for the locked lines of the TODO.md view (§16); plain Lua:
--   lua neovim/test/lock_test.lua        (from the repository root)
-- The cases are those of the plugin's `view-lock.test.ts`: the two must agree.
package.path = "neovim/lua/?.lua;" .. package.path
local lock = require("restask.lock")

local count = 0

local function eq(got, want, what)
	count = count + 1
	assert(got == want, ("%s: expected %s, got %s"):format(what, tostring(want), tostring(got)))
end

local VIEW = {
	"---", -- 1
	"restask-list: dev", -- 2
	"restask-render: 4f461dcfffcd3738", -- 3
	"---", -- 4
	"# TODO", -- 5
	"", -- 6
	"## 🔺 Highest Priority", -- 7
	"- [ ] a", -- 8
	"- [ ] b", -- 9
	"", -- 10
	"## Done", -- 11
	"- [x] c", -- 12
}

local locked = lock.locked(VIEW)
for _, row in ipairs({ 1, 2, 3, 4, 5, 7, 11 }) do
	eq(locked[row], true, "locked " .. row)
end
for _, row in ipairs({ 6, 8, 9, 10, 12 }) do
	eq(locked[row], nil, "free " .. row)
end

eq(lock.home(VIEW), 6, "home is the line under # TODO")
eq(lock.home({ "# TODO" }), nil, "nothing under the heading")
eq(lock.home({ "- [ ] a" }), nil, "no heading, no home")
eq(lock.home({ "# TODO", "", "## Done" }), 2, "home without frontmatter")

eq(lock.free(VIEW, 8, 9), 8, "a free row stays")
eq(lock.free(VIEW, 7, 6), 8, "down over a heading")
eq(lock.free(VIEW, 7, 8), 6, "up over a heading")
eq(lock.free(VIEW, 11, 10), 12, "down over the done heading")
eq(lock.free(VIEW, 5, 6), 6, "up at the top: nowhere to go, back where it was")
eq(lock.free(VIEW, 1, 8), 6, "gg lands in the first free row")
eq(lock.free(VIEW, 7, nil), 8, "no previous row reads as down")
eq(lock.free({ "# A", "## B" }, 1, nil), nil, "all locked")

local fenced = { "# TODO", "", "```", "# not a heading", "```", "- [ ] a" }
eq(lock.locked(fenced)[4], nil, "a fenced line is free")

print(("lock_test: %d checks passed"):format(count))
