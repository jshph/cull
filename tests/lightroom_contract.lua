-- Run with Lua 5.1+ from the repository root. No Lightroom or original files required.
local Manifest = dofile('integrations/Cull.lrplugin/Manifest.lua')
local Handoff = dofile('integrations/Cull.lrplugin/Handoff.lua')
local header = 'CULL\t1\t0123456789abcdef\tA%09shoot\t/photos\n'
local shoot = Manifest.parse(header .. '/photos/a.DNG\t1\t1\tGreen\t3\n/photos/b.JPG\t-1\t1\tRed\n/photos/c.DNG\t0\t0\t\n')
assert(shoot.name == 'A\tshoot' and #shoot.photos == 3)
for _, invalid in ipairs({
    '/elsewhere/a.DNG\t1\t1\t\n', '/photos/../a.DNG\t1\t1\t\n',
    '/photos/a.DNG\t2\t1\t\n', '/photos/a.DNG\t1\ttrue\t\n',
    '/photos/a.DNG\t1\t1\t\n/photos/a.DNG\t0\t1\t\n',
}) do assert(not pcall(Manifest.parse, header .. invalid)) end

local catalog = {sets={}, collections={}, photos={}, imports=0, gate=0, nextId=0}
function catalog:withWriteAccessDo(_, fn)
    assert(self.gate == 0); self.gate = 1; fn(); self.gate = 0
end
function catalog:createCollectionSet(name, parent, prior)
    assert(self.gate == 1 and prior)
    self.sets[name] = self.sets[name] or {name=name}; return self.sets[name]
end
function catalog:createCollection(name, parent, prior)
    assert(self.gate == 1 and prior)
    local key = parent.name .. '/' .. name
    if not self.collections[key] then
        local collection = {name=name, members={}}
        function collection:getPhotos() assert(catalog.gate == 0); local copy={};for _,p in pairs(self.members) do copy[#copy+1]=p end;return copy end
        function collection:addPhotos(photos) for _,p in ipairs(photos) do self.members[p.localIdentifier]=p end end
        function collection:removePhotos(photos) for _,p in ipairs(photos) do self.members[p.localIdentifier]=nil end end
        self.collections[key] = collection
    end
    return self.collections[key]
end
function catalog:createSmartCollection(name, rules, parent, prior)
    local collection = self:createCollection(name,parent,prior)
    collection.rules = collection.rules or rules; return collection
end
function catalog:findPhotoByPath(path) assert(self.gate == 0);return self.photos[path] end
function catalog:addPhoto(path)
    assert(self.gate == 1 and not self.photos[path]);self.imports=self.imports+1;self.nextId=self.nextId+1
    local photo = {localIdentifier=self.nextId, metadata={rating=4,pickStatus=0,label='Client Blue',colorNameForLabel='blue'}, properties={}}
    function photo:getRawMetadata(key) return self.metadata[key] end
    function photo:setRawMetadata(key,value)
        assert(catalog.gate == 1)
        assert(key ~= 'developSettings', 'Must preserve edits')
        if key == 'rating' then assert(self.metadata.rating == 4 and value == 3, 'Only initialize a new photo grade') end
        self.metadata[key]=value
        if key=='colorNameForLabel' then self.metadata.label = 'Custom ' .. value end
    end
    function photo:getPropertyForPlugin(_,key) return self.properties[key] end
    function photo:setPropertyForPlugin(_,key,value) assert(catalog.gate == 1);self.properties[key]=value end
    self.photos[path]=photo;return photo
end
function catalog:setActiveSources(sources) assert(self.gate == 0);self.active=sources end
local plugin = {}
assert(Handoff.apply(catalog,shoot,plugin) == 3 and catalog.imports == 3)
assert(Handoff.apply(catalog,shoot,plugin) == 3 and catalog.imports == 3)
local count=0;for _ in pairs(catalog.collections) do count=count+1 end;assert(count==4)
local a = catalog.photos['/photos/a.DNG'];local c = catalog.photos['/photos/c.DNG']
assert(a.metadata.rating==3 and a.metadata.pickStatus==1 and a.metadata.label=='Custom green')
assert(a.properties.previousLabel=='Client Blue')
-- Native rating and a decision on an untouched existing photo survive refresh.
a.metadata.rating=5;c.metadata.pickStatus=1
Handoff.apply(catalog,shoot,plugin)
assert(a.metadata.rating==5 and c.metadata.pickStatus==1)
-- Explicit unmark restores a pre-existing custom label.
shoot.photos[1].flag=0;Handoff.apply(catalog,shoot,plugin)
assert(a.metadata.pickStatus==0 and a.metadata.label=='Client Blue' and a.metadata.rating==5)
-- Removing from this shoot never deletes an original or catalog photo.
table.remove(shoot.photos,2);Handoff.apply(catalog,shoot,plugin)
assert(catalog.photos['/photos/b.JPG'])
assert(#catalog.active[1]:getPhotos()==2)
for _, collection in pairs(catalog.collections) do
    if collection.rules then
        assert(collection.rules.combine=='intersect')
        assert(collection.rules[1].criteria=='collection' and collection.rules[1].value=='cull-'..shoot.id)
        assert(collection.rules[2].criteria=='pick')
    end
end
print('Lightroom contract: manifest validation, native flags, custom labels, preserved stars, idempotence and shoot scope passed')
