-- Behaviour test for the metadata suggestions of the Neovim integration (§16, §15.2);
-- plain Lua:
--   lua neovim/test/suggest_test.lua        (from the repository root)
-- The cases are those of the plugin's `modal-filter.test.ts`: the two must agree.
package.path = "neovim/lua/?.lua;" .. package.path
local suggest = require("restask.suggest")

local TODAY = "2026-09-23"
local count = 0

local function eq(got, want, what)
	count = count + 1
	assert(got == want, ("%s: expected %q, got %q"):format(what, tostring(want), tostring(got)))
end

--- The suggestions for `fragment` as `keyword=insert` pairs, comma-separated.
local function offered(fragment)
	local out = {}
	for i, entry in ipairs(suggest.suggestions_for(fragment, TODAY)) do
		out[i] = entry.keyword .. "=" .. entry.insert
	end
	return table.concat(out, ",")
end

--- The trigger for `before` as `start:query`, or nil.
local function trigger(before)
	local start, query = suggest.trigger_at(before, TODAY)
	return start and (start .. ":" .. query) or nil
end

-- The filter.
eq(suggest.MIN_FRAGMENT_LENGTH, 2, "threshold")
eq(offered(""), "", "nothing typed")
eq(offered("h"), "", "one letter")
eq(offered("d"), "", "one letter, d")
eq(offered("hi"), "highest=🔺,high=⏫", "hi: high and highest only")
eq(offered("HI"), offered("hi"), "letter case")
eq(offered("ToDay"), "today=📅 " .. TODAY, "letter case, today")
eq(offered("highest"), "highest=🔺", "highest")
eq(offered("high"), "highest=🔺,high=⏫", "high")
eq(offered("medium"), "medium=🔼", "medium")
eq(offered("low"), "low=🔽,lowest=⏬", "low")
eq(offered("lowest"), "lowest=⏬", "lowest")
eq(offered("due"), "due=📅 ", "due")
eq(offered("start"), "start=🛫 ", "start")
eq(offered("scheduled"), "scheduled=⏳ ", "scheduled")
eq(offered("tomorrow"), "tomorrow=📅 2026-09-24", "tomorrow")
eq(offered("to"), "today=📅 " .. TODAY .. ",tomorrow=📅 2026-09-24", "to: today and tomorrow")
eq(offered("zz"), "", "no match")
eq(offered("cr"), "created=➕", "created: the bare token")
eq(offered("ca"), "calendar=📁 ", "calendar: the token, its name is typed behind it")
eq(offered("re"), "repeat=🔁 every ", "repeat")

-- Calendar arithmetic.
eq(suggest.add_days("2026-09-30", 1), "2026-10-01", "month end")
eq(suggest.add_days("2028-02-28", 1), "2028-02-29", "leap year")
eq(suggest.add_days("2026-02-28", 1), "2026-03-01", "no leap year")
eq(suggest.add_days("2026-12-31", 1), "2027-01-01", "year end")
eq(suggest.add_days("soon", 1), "soon", "no date")

-- The typing trigger.
eq(trigger("- [ ] buy milk hi"), "15:hi", "a fragment at the end of a task line")
eq(trigger("  * [x] call tom"), "13:tom", "indented, checked")
eq(trigger("- [ ] du"), "6:du", "right after the checkbox")
eq(trigger("- [ ] buy milk cre"), "15:cre", "created")
eq(trigger("- [ ] update the readme cal"), "24:cal", "calendar")
eq(trigger("- [ ] caffè 🔺 hi"), "18:hi", "the column is in bytes")
eq(trigger("plain prose hi"), nil, "prose")
eq(trigger("- not a checkbox hi"), nil, "a list item without a box")
eq(trigger("- [ ] buy milk h"), nil, "one letter")
eq(trigger("- [ ] buy milk"), nil, "no keyword")
eq(trigger("- [ ] buy milk "), nil, "a blank at the end")
eq(trigger("- [ ] delhi"), nil, "a keyword inside a longer word")
eq(trigger("- [ ] x2hi"), nil, "letters behind a digit")
eq(trigger("- [ ] highway"), nil, "a longer word")

print(("suggest_test: %d checks passed"):format(count))
