function sed --description "GNU sed → sd wrapper"
    if not command -q sd
        command sed $argv
        return $status
    end

    # Pass through --version/--help to the real tool
    for a in $argv
        if test "$a" = "--version"; or test "$a" = "--help"
            command sed $argv
            return $status
        end
    end

    set -l sd_args
    set -l files
    set -l expressions
    set -l in_place false
    set -l in_place_backup ""

    set -l i 1
    while test $i -le (count $argv)
        set -l arg $argv[$i]

        if test "$arg" = "-i"
            set in_place true
            # Check if next arg looks like a backup suffix (starts with .)
            if test (math $i + 1) -le (count $argv)
                set -l next $argv[(math $i + 1)]
                if string match -q '.*' -- $next; and not string match -q 's*' -- $next
                    set i (math $i + 1)
                    set in_place_backup $next
                end
            end
        else if string match -q -- '-i.*' $arg
            # -i.bak style (suffix attached)
            set in_place true
            set in_place_backup (string sub -s 3 -- $arg)
        else if test "$arg" = "-e"
            set i (math $i + 1)
            set -a expressions $argv[$i]
        else if test "$arg" = "-n"
            # -n (suppress output) — no sd equivalent, fall back to real sed
            command sed $argv
            return $status
        else if test "$arg" = "-E"; or test "$arg" = "-r"; or test "$arg" = "--regexp-extended"
            # ERE mode — sd uses this by default, skip
        else if string match -q 's*' -- $arg; and test (string length -- $arg) -gt 1
            # Detect s/pattern/replacement/ syntax
            set -l second_char (string sub -s 2 -l 1 -- $arg)
            if string match -qr '[^a-zA-Z0-9]' -- $second_char
                set -a expressions $arg
            else
                # Not a sed expression, treat as file/arg
                set -a files $arg
            end
        else
            set -a files $arg
        end

        set i (math $i + 1)
    end

    # If no expressions found, fall back to real sed
    if test (count $expressions) -eq 0
        command sed $argv
        return $status
    end

    # Process expressions — convert s/find/replace/flags to sd args
    for expr in $expressions
        set -l delim (string sub -s 2 -l 1 -- $expr)
        set -l rest (string sub -s 3 -- $expr)

        # Split on delimiter — need to handle this carefully
        set -l parts (string split -- $delim $rest)

        if test (count $parts) -lt 2
            # Can't parse, fall back
            echo "sed wrapper: can't parse '$expr' — falling back to GNU sed" >&2
            command sed $argv
            return $status
        end

        set -l find_pat $parts[1]
        set -l replace_pat $parts[2]
        set -l flags ""
        if test (count $parts) -ge 3
            set flags $parts[3]
        end

        # Convert BRE find pattern to ERE (what sd expects):
        # \( → (  and  \) → )
        set find_pat (string replace -a -- '\\(' '(' $find_pat)
        set find_pat (string replace -a -- '\\)' ')' $find_pat)
        # \+ → +  \| → |  \{ → {  \} → }
        set find_pat (string replace -a -- '\\+' '+' $find_pat)
        set find_pat (string replace -a -- '\\|' '|' $find_pat)
        set find_pat (string replace -a -- '\\{' '{' $find_pat)
        set find_pat (string replace -a -- '\\}' '}' $find_pat)

        # Convert replacement backrefs: \1 → $1, \2 → $2, etc.
        set replace_pat (string replace -a -- '\\1' '$1' $replace_pat)
        set replace_pat (string replace -a -- '\\2' '$2' $replace_pat)
        set replace_pat (string replace -a -- '\\3' '$3' $replace_pat)
        set replace_pat (string replace -a -- '\\4' '$4' $replace_pat)
        set replace_pat (string replace -a -- '\\5' '$5' $replace_pat)
        set replace_pat (string replace -a -- '\\6' '$6' $replace_pat)
        set replace_pat (string replace -a -- '\\7' '$7' $replace_pat)
        set replace_pat (string replace -a -- '\\8' '$8' $replace_pat)
        set replace_pat (string replace -a -- '\\9' '$9' $replace_pat)

        # Convert & → $0 (whole match ref), but not \& (literal &)
        # First protect escaped \& by replacing with a placeholder
        set replace_pat (string replace -a -- '\\&' '\x00AMP' $replace_pat)
        # Now replace bare & with $0
        set replace_pat (string replace -a -- '&' '$0' $replace_pat)
        # Restore literal &
        set replace_pat (string replace -a -- '\x00AMP' '&' $replace_pat)

        # Handle flags
        if string match -q '*i*' -- $flags
            set -a sd_args -f i
        end

        set -a sd_args $find_pat $replace_pat
    end

    # Handle backup before in-place edit
    if test $in_place = true; and test -n "$in_place_backup"
        for f in $files
            cp -- $f "$f$in_place_backup"
        end
    end

    # Execute sd
    if test (count $files) -gt 0
        if test $in_place = true
            # sd modifies files in-place by default when given file args
            command sd $sd_args $files
        else
            # No -i flag: pipe file through sd to stdout (don't modify original)
            for f in $files
                command sd $sd_args <$f
            end
        end
    else
        # Stdin mode
        command sd $sd_args
    end
end
