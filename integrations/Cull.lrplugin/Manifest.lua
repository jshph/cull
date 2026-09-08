local M = {}
function M.decode(value)
    return (value:gsub('%%(%x%x)', function(hex) return string.char(tonumber(hex, 16)) end))
end
local function columns(line)
    local result = {}
    for field in (line .. '\t'):gmatch('(.-)\t') do result[#result + 1] = M.decode(field) end
    return result
end
function M.parse(text)
    assert(#text <= 32 * 1024 * 1024, 'Cull manifest is too large')
    local lines = {}
    for line in text:gmatch('[^\n]+') do lines[#lines + 1] = line:gsub('\r$', '') end
    local header = columns(lines[1] or '')
    assert(#header == 5 and header[1] == 'CULL' and header[2] == '1', 'Not a supported Cull shoot')
    assert(#header[3] == 16 and header[3]:match('^%x+$'), 'Invalid shoot identity')
    local root = header[5]:gsub('/+$', '')
    assert(root:sub(1,1) == '/' and not root:match('/%.%./') and root:sub(-3) ~= '/..', 'Invalid originals folder')
    local result = { id = header[3], name = header[4], root = root, photos = {} }
    local seen = {}
    for i = 2, #lines do
        local row = columns(lines[i]); local flag = tonumber(row[2])
        assert((#row == 4 or #row == 5) and (flag == -1 or flag == 0 or flag == 1), 'Invalid photo decision')
        assert(row[3] == '0' or row[3] == '1', 'Invalid ownership marker')
        assert(row[1]:sub(1,#root+1) == root .. '/' and not row[1]:match('/%.%./'), 'Photo is outside this shoot')
        assert(not seen[row[1]], 'Duplicate photo in manifest'); seen[row[1]] = true
        local rating = row[5] and row[5] ~= '' and tonumber(row[5]) or nil
        assert(not row[5] or row[5] == '' or (rating and rating >= 0 and rating <= 5 and rating == math.floor(rating)), 'Invalid star rating')
        result.photos[#result.photos+1] = { path = row[1], flag = flag, authored = row[3] == '1', label = row[4], rating = rating }
    end
    return result
end
return M
