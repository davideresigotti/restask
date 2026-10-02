-- Behaviour test for the new-line rule of the Neovim integration (§16, §15.7); plain Lua:
--   lua neovim/test/start_test.lua        (from the repository root)
-- The cases are those of the plugin's `task-start.test.ts`: the two must agree.
package.path = "neovim/lua/?.lua;" .. package.path
local start = require("restask.start")

local count = 0

local function eq(got, want, what)
	count = count + 1
	assert(got == want, ("%s: expected %q, got %q"):format(what, tostring(want), tostring(got)))
end

local function split(text)
	local lines = {}
	for line in (text .. "\n"):gmatch("(.-)\n") do
		lines[#lines + 1] = line
	end
	return lines
end

--- For each line of `text`, whether it lies in the TODO section: `+` or `-`.
local function sections(text, view, done)
	local lines = split(text)
	local out = {}
	for row = 1, #lines do
		out[row] = start.in_todo(lines, row, view, done or "Done") and "+" or "-"
	end
	return table.concat(out)
end

local A = " 🆔 restask-01jzq4tsvg2c9xkw7n5m8rhdpb"
local VIEW = "---\nrestask-list: dev\nrestask-render: 0000000000000000\n---\n# TODO\n\n"

-- The TODO section of a note.
eq(
	sections("---\nrestask-list: Home\n---\n# Home\nprose\n\n# TODO\n- [ ] a\n\n## Done\n- [x] b\n", false),
	"-------+++--",
	"from below the TODO heading to the done heading"
)
eq(sections("## TODO\n\n### Later\n\n## Notes\n\n# Top\n", false), "-++++---", "ends at a heading of the same level")
eq(sections("## TODO\n\n# Top\n\n", false), "-++--", "ends at a higher heading")
eq(sections("# Todo\n\n", false), "-++", "any letter case")
eq(sections("# todo ##\n\n", false), "-++", "closing hashes")
eq(sections("# TODO list\n\n", false), "---", "no other text")
eq(sections("TODO\n\n", false), "---", "no heading")
eq(sections("# TODO\n\n# Notes\n\n# TODO\n\n", false), "-++--++", "a second TODO heading")
eq(sections("# TODO\n\n# Done\n\n# TODO\n\n", false), "-++----", "never below the done heading")
eq(sections("# TODO\n\n# Finished\n\n", false, "Finished"), "-++--", "the vault's done heading")
eq(sections("---\ntitle: TODO\n---\n\n# TODO\n", false), "-----+", "frontmatter")
eq(sections("# TODO\n```\n# Notes\n\n```\n\n", false), "-+---++", "a fenced block inside")
eq(sections("```\n# TODO\n```\n\n", false), "-----", "a heading in a fenced block")
eq(sections("# Home\n- [ ] a\n\n", false), "----", "a note without the heading")

-- The TODO section of the view.
eq(
	sections(VIEW .. "## 🔺 Highest Priority\n- [ ] a 🔺" .. A .. "\n\n## No Priority\n\n## Done\n- [x] b\n", true),
	"----++++++++--",
	"the whole body above Done"
)
eq(
	sections("---\nrestask-list: dev\n---\n# Inbox\n\n## Done\n\n", true, "Finished"),
	"---+++--",
	"whatever the title and the vault's done heading"
)

-- The line that is started.
eq(start.started(""), "- [ ] ", "an empty line")
eq(start.started("\t"), "\t- [ ] ", "the indentation is kept")
eq(start.started("    "), "    - [ ] ", "blanks too")
eq(start.started("- [ ] "), nil, "a line with a checkbox")
eq(start.started("prose"), nil, "a line with text")

-- Which notes take part.
eq(start.routes_itself(split("---\nrestask-list: Home\n---\n# TODO")), true, "restask-list")
eq(start.routes_itself(split("---\ntags: x\nrestask-list-root:  Home Lab \n---\n")), true, "restask-list-root")
eq(start.routes_itself(split("---\nrestask-list:\n---\n")), false, "an empty value")
eq(start.routes_itself(split("---\ntags: x\n---\n# TODO")), false, "other frontmatter")
eq(start.routes_itself(split("# TODO\nrestask-list: Home")), false, "no frontmatter")
eq(start.routes_itself(split("---\nrestask-list: Home\n# TODO")), false, "an unterminated block")
eq(start.declares_root(split("---\nrestask-list-root: Home Lab\n---\n")), true, "a root note")
eq(start.declares_root(split("---\nrestask-list: Home\n---\n")), false, "a routed note is no root")

-- The vault config.
local cfg = start.config('done_heading = "Finished"\ninbox_file = "Inbox/Tasks.md"\ninbox_list = "dev"\n')
eq(cfg.inbox_file, "Inbox/Tasks.md", "inbox_file")
eq(cfg.done_heading, "Finished", "done_heading")
cfg = start.config('inbox_list = "dev"\n')
eq(cfg.inbox_file, "TODO.md", "default inbox file")
eq(cfg.done_heading, "Done", "default done heading")
cfg = start.config(nil)
eq(cfg.inbox_file, nil, "no config, no view")
eq(cfg.done_heading, "Done", "no config, default done heading")

print(("start_test: %d checks passed"):format(count))
