local LrTasks = import 'LrTasks'
local LrDialogs = import 'LrDialogs'
local LrFileUtils = import 'LrFileUtils'
local LrPathUtils = import 'LrPathUtils'
local LrApplication = import 'LrApplication'
local Manifest = dofile(LrPathUtils.child(_PLUGIN.path, 'Manifest.lua'))
local Handoff = dofile(LrPathUtils.child(_PLUGIN.path, 'Handoff.lua'))
return function(path)
    LrTasks.startAsyncTask(function()
        local ok, err = LrTasks.pcall(function()
            if not path then
                local selected = LrDialogs.runOpenPanel({title='Choose the shoot’s .cull/handoff.cull file', canChooseFiles=true, canChooseDirectories=false, allowsMultipleSelection=false})
                if not selected then return end
                path = selected[1]
            end
            local shoot = Manifest.parse(assert(LrFileUtils.readFile(path), 'Cannot read Cull shoot'))
            for _, row in ipairs(shoot.photos) do
                assert(LrFileUtils.exists(row.path) == 'file', 'Photo is missing: ' .. row.path)
            end
            local count = Handoff.apply(LrApplication.activeCatalog(), shoot, _PLUGIN)
            local receipt = io.open(path .. '.receipt', 'w')
            if receipt then receipt:write('OK\t' .. tostring(count)); receipt:close() end
            LrDialogs.message('Cull — ' .. shoot.name, tostring(count) .. ' originals ready. Picks, Rejects and Unmarked update as you change flags. Existing star ratings and edits were preserved.', 'info')
        end)
        if not ok then LrDialogs.message('Cull handoff failed', tostring(err), 'critical') end
    end)
end
