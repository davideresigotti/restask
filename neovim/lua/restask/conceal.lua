-- restask conceal (§16): keeps the `🆔 restask-…` token of task lines off the screen.
-- The token stays in the file — it links the line to its task on the server; only the
-- display changes, through a window match and the window's conceal options.
local M = {}

--- The token of §6.1 with the blanks before it, as a Vim pattern.
M.PATTERN = [[\s*🆔\s\+\%(restask\|taskres\)-[0-9a-z]\{26}]]

--- Modes in which the cursor line is concealed too; they are added to the window's
-- 'concealcursor'. Insert mode is left out on purpose: the line being typed in shows its
-- token, so it is not overwritten blindly.
M.concealcursor = "nc"

--- `current` with the modes of `wanted` it lacks appended.
---@param current string
---@param wanted string
---@return string
function M.with_modes(current, wanted)
	local out = current
	for mode in wanted:gmatch(".") do
		if not out:find(mode, 1, true) then
			out = out .. mode
		end
	end
	return out
end

--- True when `file` lies in a restask vault (a `restask.toml` or `.restask/` above it).
---@param file string absolute path
---@return boolean
function M.in_vault(file)
	if file == "" then
		return false
	end
	local found = vim.fs.find({ "restask.toml", ".restask" }, { upward = true, path = vim.fs.dirname(file), limit = 1 })
	return #found > 0
end

--- Conceals the tokens in the current window when it shows a Markdown file of a vault,
-- and undoes that when it shows anything else. 'conceallevel' is raised to 2 and
-- 'concealcursor' gains the modes above; both get their previous value back, unless
-- something else changed them in the meantime.
function M.refresh()
	local buf = vim.api.nvim_get_current_buf()
	local wanted = vim.bo[buf].filetype == "markdown" and M.in_vault(vim.api.nvim_buf_get_name(buf))
	local state = vim.w.restask_conceal
	if wanted and not state then
		state = {
			match = vim.fn.matchadd("Conceal", M.PATTERN, 10, -1, { conceal = "" }),
			level = vim.wo.conceallevel,
			cursor = vim.wo.concealcursor,
		}
		vim.wo.conceallevel = math.max(state.level, 2)
		vim.wo.concealcursor = M.with_modes(state.cursor, M.concealcursor)
		state.set_level = vim.wo.conceallevel
		state.set_cursor = vim.wo.concealcursor
		vim.w.restask_conceal = state
	elseif state and not wanted then
		pcall(vim.fn.matchdelete, state.match)
		if vim.wo.conceallevel == state.set_level then
			vim.wo.conceallevel = state.level
		end
		if vim.wo.concealcursor == state.set_cursor then
			vim.wo.concealcursor = state.cursor
		end
		vim.w.restask_conceal = nil
	end
end

--- Registers the autocommands that keep every window up to date.
---@param concealcursor string|nil modes to add to 'concealcursor' (default "nc"; "" adds none)
function M.register(concealcursor)
	if concealcursor ~= nil then
		M.concealcursor = concealcursor
	end
	local group = vim.api.nvim_create_augroup("restask_conceal", { clear = true })
	vim.api.nvim_create_autocmd({ "BufWinEnter", "WinEnter", "FileType" }, {
		group = group,
		callback = M.refresh,
	})
	M.refresh()
end

return M
