local LrPathUtils = import 'LrPathUtils'
local Manifest = dofile(LrPathUtils.child(_PLUGIN.path, 'Manifest.lua'))
return { URLHandler = function(url)
    local path = url:match('^lightroom://com%.getcull%.shoot/open%?manifest=([^&]+)$')
    if not path then error('Invalid Cull handoff URL') end
    dofile(LrPathUtils.child(_PLUGIN.path, 'Run.lua'))(Manifest.decode(path))
end }
