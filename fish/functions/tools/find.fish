function find --description "GNU find → fd wrapper"
    if not command -q fd
        command find $argv
        return $status
    end

    # Pass through --version/--help to the real tool
    for a in $argv
        if test "$a" = "--version"; or test "$a" = "--help"
            command find $argv
            return $status
        end
    end

    set -l fd_args
    set -l search_paths
    set -l has_pattern false
    set -l exec_args
    set -l in_exec false
    set -l found_flag false

    set -l i 1
    while test $i -le (count $argv)
        set -l arg $argv[$i]

        # If we're collecting -exec arguments
        if test $in_exec = true
            if test "$arg" = ";"
                set in_exec false
                set -a fd_args -x $exec_args
                set exec_args
            else if test "$arg" = "+"
                set in_exec false
                set -a fd_args -X $exec_args
                set exec_args
            else if test "$arg" != "{}"
                set -a exec_args $arg
            end
            set i (math $i + 1)
            continue
        end

        # In GNU find, paths come before any flags
        if not $found_flag; and not string match -qr '^-' -- $arg; and not test "$arg" = "!"
            set -a search_paths $arg
            set i (math $i + 1)
            continue
        end

        set found_flag true

        if test "$arg" = "-name"
            set i (math $i + 1)
            set has_pattern true
            set -l nameglob $argv[$i]
            if string match -qr '^\*\.\w+$' -- $nameglob
                set -a fd_args -e (string replace -- '*.' '' $nameglob)
            else
                set -a fd_args --glob $nameglob
            end
        else if test "$arg" = "-iname"
            set i (math $i + 1)
            set has_pattern true
            set -a fd_args --glob --ignore-case $argv[$i]
        else if test "$arg" = "-type"
            set i (math $i + 1)
            set -a fd_args -t $argv[$i]
        else if test "$arg" = "-maxdepth"
            set i (math $i + 1)
            set -a fd_args -d $argv[$i]
        else if test "$arg" = "-mindepth"
            set i (math $i + 1)
            set -a fd_args --min-depth $argv[$i]
        else if test "$arg" = "-mtime"
            set i (math $i + 1)
            set -l days $argv[$i]
            if string match -qr '^\+' -- $days
                set -l n (string replace -- '+' '' $days)
                set -a fd_args --changed-before {$n}d
            else if string match -qr '^\-' -- $days
                set -l n (string replace -- '-' '' $days)
                set -a fd_args --changed-within {$n}d
            else
                set -a fd_args --changed-before {$days}d
            end
        else if test "$arg" = "-size"
            set i (math $i + 1)
            set -a fd_args -S $argv[$i]
        else if test "$arg" = "-exec"
            set in_exec true
        else if test "$arg" = "-delete"
            set -a fd_args -X rm -rf
        else if test "$arg" = "-print"
            # no-op
        else if test "$arg" = "-print0"
            set -a fd_args -0
        else if test "$arg" = "-empty"
            set -a fd_args -t e
        else if test "$arg" = "-L"; or test "$arg" = "-follow"
            set -a fd_args -L
        else if test "$arg" = "-not"; or test "$arg" = "!"
            set i (math $i + 1)
            if test "$argv[$i]" = "-name"
                set i (math $i + 1)
                set -a fd_args -E $argv[$i]
            end
        else if test "$arg" = "-regex"
            set i (math $i + 1)
            set has_pattern true
            set -a fd_args $argv[$i]
        else if test "$arg" = "-path"
            set i (math $i + 1)
            set has_pattern true
            set -a fd_args --full-path $argv[$i]
        end

        set i (math $i + 1)
    end

    # Default to current dir if no paths given
    if test (count $search_paths) -eq 0
        set search_paths .
    end

    # fd syntax: fd [OPTIONS] [pattern] [path...]
    # When we have search paths that aren't "." and no explicit regex pattern,
    # fd needs a match-all pattern "." to avoid interpreting the path as a pattern
    if test "$search_paths" != "."
        command fd $fd_args . $search_paths
    else
        command fd $fd_args
    end
end
