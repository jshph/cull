local M = {}
-- Separate from the UI entry point so the catalog contract can be tested.
function M.apply(catalog, shoot, plugin)
    local set, originals
    local sourceName = 'Originals [cull-' .. shoot.id .. ']'
    catalog:withWriteAccessDo('Create Cull shoot', function()
        set = assert(catalog:createCollectionSet(shoot.name .. ' [Cull ' .. shoot.id .. ']', nil, true))
    end)
    catalog:withWriteAccessDo('Create Cull views', function()
        originals = assert(catalog:createCollection(sourceName, set, true))
        for _, state in ipairs({{'Picks',1},{'Rejects',-1},{'Unmarked',0}}) do
            catalog:createSmartCollection(state[1], {
                combine = 'intersect',
                { criteria = 'collection', operation = 'all', value = 'cull-' .. shoot.id },
                { criteria = 'pick', operation = '==', value = state[2] },
            }, set, true)
        end
    end)
    local photos, newlyAdded = {}, {}
    -- Discover before the write gate; never import a duplicate original.
    for i, row in ipairs(shoot.photos) do photos[i] = catalog:findPhotoByPath(row.path) or false end
    catalog:withWriteAccessDo('Import Cull originals in place', function()
        for i, row in ipairs(shoot.photos) do
            if not photos[i] then
                photos[i] = assert(catalog:addPhoto(row.path), 'Could not import ' .. row.path)
                newlyAdded[i] = true
            end
        end
    end)
    local previousMembers = originals:getPhotos()
    local wanted = {}; for _, photo in ipairs(photos) do wanted[photo.localIdentifier] = true end
    local removed = {}; for _, photo in ipairs(previousMembers) do
        if not wanted[photo.localIdentifier] then removed[#removed+1] = photo end
    end
    catalog:withWriteAccessDo('Apply Cull decisions', function()
        for i, row in ipairs(shoot.photos) do
            local photo = photos[i]
            if newlyAdded[i] and row.rating ~= nil then photo:setRawMetadata('rating', row.rating) end
            if row.authored or newlyAdded[i] then
                local label = photo:getRawMetadata('label') or ''
                local color = photo:getRawMetadata('colorNameForLabel') or 'none'
                local applied = photo:getPropertyForPlugin(plugin, 'appliedLabel')
                local prior = photo:getPropertyForPlugin(plugin, 'previousLabel')
                if row.flag ~= 0 then
                    if label ~= applied then
                        prior = (color ~= 'green' and color ~= 'red') and label or ''
                        photo:setPropertyForPlugin(plugin, 'previousLabel', prior)
                    end
                    photo:setRawMetadata('colorNameForLabel', row.flag == 1 and 'green' or 'red')
                    photo:setPropertyForPlugin(plugin, 'appliedLabel', photo:getRawMetadata('label') or '')
                elseif label == applied or color == 'green' or color == 'red' then
                    photo:setRawMetadata('label', (prior and prior ~= '') and prior or row.label or '')
                    photo:setPropertyForPlugin(plugin, 'appliedLabel', nil)
                    photo:setPropertyForPlugin(plugin, 'previousLabel', nil)
                end
                photo:setRawMetadata('pickStatus', row.flag)
                -- Never overwrite an existing catalog rating, keywords, develop settings,
                -- or Save Metadata to File: DNG/JPEG originals stay untouched.
            end
        end
        if #removed > 0 then originals:removePhotos(removed) end
        if #photos > 0 then originals:addPhotos(photos) end
    end)
    catalog:setActiveSources({ originals })
    return #photos
end
return M
