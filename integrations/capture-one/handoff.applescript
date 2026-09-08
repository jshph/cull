use framework "Foundation"
use scripting additions
on run argv
    set configPath to item 1 of argv
    set configData to current application's NSData's dataWithContentsOfFile:configPath
    set config to current application's NSJSONSerialization's JSONObjectWithData:configData options:0 |error|:(missing value)
    if config is missing value then error "Cannot read Cull handoff configuration"
    set sessionName to (config's objectForKey:"name") as text
    set sessionContainer to (config's objectForKey:"container") as text
    set sessionFile to (config's objectForKey:"session") as text
    set sourceRoot to (config's objectForKey:"originals") as text
    set exportRoot to (config's objectForKey:"exports") as text
    set sourceFolders to (config's objectForKey:"folders") as list
    set decisions to config's objectForKey:"decisions"
    set photoInventory to config's objectForKey:"inventory"
    set folderCounts to config's objectForKey:"folderCounts"
    set albumRules to config's objectForKey:"rules"
    set labelsPath to (config's objectForKey:"labels") as text
    set priorLabels to current application's NSMutableDictionary's dictionary()
    set labelsData to current application's NSData's dataWithContentsOfFile:labelsPath
    if labelsData is not missing value then
        set priorLabels to current application's NSJSONSerialization's JSONObjectWithData:labelsData options:1 |error|:(missing value)
        if priorLabels is missing value then error "Cannot read saved Capture One labels"
    end if
    set existsAlready to (current application's NSFileManager's defaultManager()'s fileExistsAtPath:sessionFile) as boolean
    if existsAlready then set sessionAlias to (POSIX file sessionFile) as alias
    tell application __EDITOR__
        activate
        with timeout of 600 seconds
            if existsAlready then
                set shootDoc to missing value
                repeat with candidate in documents
                    if (POSIX path of (folder of candidate as alias)) is (sessionContainer & "/" & sessionName) then set shootDoc to contents of candidate
                end repeat
                if shootDoc is missing value then
                    open sessionAlias
                    set shootDoc to current document
                end if
                set current document to shootDoc
            else
                set shootDoc to make new document with properties {name:sessionName, path:sessionContainer, kind:session}
            end if
            set actualFolder to (current application's NSString's stringWithString:(POSIX path of (folder of shootDoc as alias)))'s stringByStandardizingPath()
            set expectedFolder to sessionContainer & "/" & sessionName
            if (actualFolder as text) is not expectedFolder then error "Capture One did not open the requested shoot session: " & (actualFolder as text)
            set captures of shootDoc to sourceRoot
            set output of shootDoc to exportRoot
            set appliedCount to 0
            set seenPaths to current application's NSMutableSet's |set|()
            repeat with sourceFolder in sourceFolders
                set folderPath to sourceFolder as text
                tell shootDoc
                    set favorites to every collection whose kind is favorite
                    set foundFolder to false
                    repeat with favoriteItem in favorites
                        try
                            set favoritePath to (current application's NSString's stringWithString:(POSIX path of (folder of favoriteItem as alias)))'s stringByStandardizingPath()
                            if (favoritePath as text) is folderPath then set foundFolder to true
                        end try
                    end repeat
                    if not foundFolder then make new collection with properties {name:folderPath, kind:favorite, folder:folderPath}
                end tell
                browse shootDoc to path folderPath
                set expectedCount to (folderCounts's objectForKey:folderPath) as integer
                repeat with attempt from 1 to 120
                    set sourceVariants to variants of current collection of shootDoc
                    set readyPaths to current application's NSMutableSet's |set|()
                    repeat with candidateVariant in sourceVariants
                        set candidatePath to path of parent image of candidateVariant as text
                        if (photoInventory's objectForKey:candidatePath) is not missing value then readyPaths's addObject:candidatePath
                    end repeat
                    if (readyPaths's |count|() as integer) is expectedCount then exit repeat
                    if attempt is 120 then error "Capture One did not expose all originals in " & folderPath & ". Check format support and folder access."
                    delay 0.25
                end repeat
                repeat with photoVariant in sourceVariants
                    set photoPath to path of parent image of photoVariant as text
                    set desiredColor to decisions's objectForKey:photoPath
                    if desiredColor is not missing value then
                        set oldColor to color tag of photoVariant as integer
                        set nextColor to desiredColor as integer
                        set labelKey to photoPath & tab & (id of photoVariant as text)
                        if nextColor is 1 or nextColor is 4 then
                            if oldColor is not 1 and oldColor is not 4 then
                                priorLabels's setObject:oldColor forKey:labelKey
                                set savedData to current application's NSJSONSerialization's dataWithJSONObject:priorLabels options:0 |error|:(missing value)
                                if not (savedData's writeToFile:labelsPath atomically:true) then error "Cannot save prior Capture One labels"
                            end if
                        else
                            set priorColor to priorLabels's objectForKey:labelKey
                            if oldColor is not 0 and oldColor is not 1 and oldColor is not 4 then
                                set nextColor to oldColor
                            else if priorColor is not missing value then
                                set nextColor to priorColor as integer
                            end if
                        end if
                        set color tag of photoVariant to nextColor
                        seenPaths's addObject:photoPath
                        set appliedCount to appliedCount + 1
                    end if
                end repeat
            end repeat
            if (seenPaths's |count|() as integer) < (decisions's |count|() as integer) then
                error "Some decided photos were not exposed by Capture One. Check format support and retry once the folder has loaded."
            end if
            repeat with albumName in {"Picks", "Rejects", "Unmarked"}
                set ruleText to (albumRules's objectForKey:(albumName as text)) as text
                tell shootDoc
                    if not (exists collection (albumName as text)) then
                        make new collection with properties {name:(albumName as text), kind:smart album, rules:ruleText}
                    end if
                end tell
            end repeat
            set current collection of shootDoc to collection "Picks" of shootDoc
            return "Opened shoot in Capture One; refreshed " & appliedCount & " variants"
        end timeout
    end tell
end run
