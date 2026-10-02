-- restask suggest (§16): the metadata suggestions of the Obsidian plugin (§15.2) while
-- typing in a task line — `hi` offers high and highest, `du` the due token, and so on.
-- The functions down to `M.trigger_at` are pure — strings in, values out, no `vim` — and
-- run under the plain interpreter (`lua neovim/test/suggest_test.lua`); what follows
-- them opens the menu of blink.cmp, whose source is `blink.lua`.
local M = {}

--- Nothing matches until this many letters are typed (§15.2).
M.MIN_FRAGMENT_LENGTH = 2

--- The id of the blink.cmp provider.
M.PROVIDER = "restask"

local PRIORITIES = {
	{ keyword = "highest", insert = "🔺" },
	{ keyword = "high", insert = "⏫" },
	{ keyword = "medium", insert = "🔼" },
	{ keyword = "low", insert = "🔽" },
	{ keyword = "lowest", insert = "⏬" },
}

local FIELDS = {
	{ keyword = "due", insert = "📅 " },
	{ keyword = "start", insert = "🛫 " },
	{ keyword = "scheduled", insert = "⏳ " },
	{ keyword = "repeat", insert = "🔁 every " },
	-- Bare: the date is written when the line is settled (§6.4).
	{ keyword = "created", insert = "➕" },
}

--- `date` (`YYYY-MM-DD`) moved by `days` calendar days; returned as it is when it is no
-- date.
---@param date string
---@param days integer
---@return string
function M.add_days(date, days)
	local y, m, d = date:match("^(%d%d%d%d)-(%d%d)-(%d%d)$")
	if not y then
		return date
	end
	-- Noon: no daylight-saving change moves it to another day.
	local shifted = os.time({ year = tonumber(y), month = tonumber(m), day = tonumber(d) + days, hour = 12 })
	return os.date("%Y-%m-%d", shifted)
end

--- The entries whose keyword starts with `fragment`, in any letter case: a list of
-- `{ keyword, insert }`. Empty below `MIN_FRAGMENT_LENGTH` letters and when nothing
-- matches. `today` is the device-local date `YYYY-MM-DD`.
---@param fragment string
---@param today string
---@return table[]
function M.suggestions_for(fragment, today)
	local query = fragment:lower()
	local found = {}
	if #query < M.MIN_FRAGMENT_LENGTH then
		return found
	end
	local dated = {
		{ keyword = "today", insert = "📅 " .. today },
		{ keyword = "tomorrow", insert = "📅 " .. M.add_days(today, 1) },
	}
	for _, group in ipairs({ PRIORITIES, FIELDS, dated }) do
		for _, entry in ipairs(group) do
			if entry.keyword:sub(1, #query) == query then
				found[#found + 1] = entry
			end
		end
	end
	return found
end

--- Whether suggestions open while typing (§15.2). `before` is the current line up to
-- the cursor. They open only inside a task line, when the cursor ends a whole word of
-- at least `MIN_FRAGMENT_LENGTH` letters that some keyword starts with. Returns the
-- 0-based byte column at which the word starts and the word, or nil.
---@param before string
---@param today string
---@return integer|nil, string|nil
function M.trigger_at(before, today)
	local body = before:match("^[ \t]*[-*+][ \t]+%[[ xX]%][ \t]+(.*)$")
	local query = body and body:match("([A-Za-z]+)$")
	if not query then
		return nil
	end
	local head = body:sub(1, #body - #query)
	if head ~= "" and not head:find("[ \t]$") then
		return nil
	end
	if #M.suggestions_for(query, today) == 0 then
		return nil
	end
	return #before - #query, query
end

--- Device-local calendar date `YYYY-MM-DD` — the wall clock is read only here.
---@return string
function M.today()
	return os.date("%Y-%m-%d")
end

--- Whether each buffer is a Markdown file of a vault, once asked, by buffer and name.
local notes = {}

--- True when `buf` is a Markdown file of a vault: suggestions are offered there, as the
-- token is concealed there.
---@param buf integer
---@return boolean
function M.in_note(buf)
	if vim.bo[buf].filetype ~= "markdown" then
		return false
	end
	local name = vim.api.nvim_buf_get_name(buf)
	local known = notes[buf]
	if not known or known.name ~= name then
		known = { name = name, vault = require("restask.conceal").in_vault(name) }
		notes[buf] = known
	end
	return known.vault
end

--- blink.cmp with the restask provider registered, or nil when blink.cmp is not
-- installed. The provider is added on first use — by then blink.cmp has been set up —
-- unless the user's own configuration defines one under that id.
local function blink()
	local ok, cmp = pcall(require, "blink.cmp")
	if not ok or type(cmp.show) ~= "function" then
		return nil
	end
	local has_config, config = pcall(require, "blink.cmp.config")
	if not has_config or not config.sources or not config.sources.providers then
		return nil
	end
	if config.sources.providers[M.PROVIDER] == nil then
		if not pcall(cmp.add_source_provider, M.PROVIDER, { name = "restask", module = "restask.blink" }) then
			return nil
		end
	end
	return cmp
end

--- Looks at the cursor line of `buf` after a change made in insert mode: when the
-- cursor ends a keyword fragment in a task line of a vault note, the menu of blink.cmp
-- is opened with the restask entries alone — also where blink.cmp is configured to show
-- nothing by itself. A menu that is open already is left to whoever opened it.
---@param buf integer
function M.offer(buf)
	if buf ~= vim.api.nvim_get_current_buf() or vim.api.nvim_get_mode().mode:sub(1, 1) ~= "i" then
		return
	end
	local col = vim.api.nvim_win_get_cursor(0)[2]
	if not M.trigger_at(vim.api.nvim_get_current_line():sub(1, col), M.today()) or not M.in_note(buf) then
		return
	end
	local cmp = blink()
	if cmp and not cmp.is_menu_visible() then
		cmp.show({ providers = { M.PROVIDER } })
	end
end

--- Registers the autocommands behind the suggestions.
function M.register()
	local group = vim.api.nvim_create_augroup("restask_suggest", { clear = true })
	vim.api.nvim_create_autocmd("TextChangedI", {
		group = group,
		callback = function(event)
			M.offer(event.buf)
		end,
	})
	vim.api.nvim_create_autocmd({ "BufUnload", "BufWipeout" }, {
		group = group,
		callback = function(event)
			notes[event.buf] = nil
		end,
	})
end

return M
