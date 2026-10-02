-- restask source for blink.cmp (§16): the metadata suggestions of §15.2 as completion
-- items. `suggest.lua` decides what is offered and registers this module as the
-- provider `restask`; choosing an entry replaces the typed fragment by its token.
local suggest = require("restask.suggest")

local source = {}

--- Called by blink.cmp once per provider.
function source.new()
	return setmetatable({}, { __index = source })
end

--- Offered in Markdown files of a vault only.
function source:enabled()
	return suggest.in_note(vim.api.nvim_get_current_buf())
end

--- The entries for the keyword fragment in front of the cursor, each replacing the
-- fragment (byte columns: blink.cmp reads the edits of a source without a language
-- server as UTF-8). Asked again on every keystroke: the range moves with the cursor.
function source:get_completions(ctx, callback)
	local row, col = ctx.cursor[1], ctx.cursor[2]
	local start, query = suggest.trigger_at(ctx.line:sub(1, col), suggest.today())
	local items = {}
	if start then
		local kind = vim.lsp.protocol.CompletionItemKind.EnumMember
		for i, entry in ipairs(suggest.suggestions_for(query, suggest.today())) do
			items[i] = {
				label = entry.keyword,
				filterText = entry.keyword,
				-- The order of §15.2's table, whatever the fuzzy score says.
				sortText = ("%02d"):format(i),
				kind = kind,
				labelDetails = { description = vim.trim(entry.insert) },
				textEdit = {
					newText = entry.insert,
					range = {
						start = { line = row - 1, character = start },
						["end"] = { line = row - 1, character = col },
					},
				},
			}
		end
	end
	callback({ items = items, is_incomplete_forward = true, is_incomplete_backward = true })
	return function() end
end

return source
