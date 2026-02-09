function du --description "GNU du → dust wrapper"
    if not command -q dust
        command du $argv
        return $status
    end

    set -l dust_args
    set -l paths

    set -l i 1
    while test $i -le (count $argv)
        set -l arg $argv[$i]

        if test "$arg" = "-h"; or test "$arg" = "--human-readable"
            # dust default, no-op
        else if test "$arg" = "-s"; or test "$arg" = "--summarize"
            set -a dust_args -d 1
        else if test "$arg" = "-d"; or test "$arg" = "--max-depth"
            set i (math $i + 1)
            set -a dust_args -d $argv[$i]
        else if string match -qr '^-d[0-9]' -- $arg
            set -a dust_args -d (string sub -s 3 -- $arg)
        else if string match -qr '^--max-depth=' -- $arg
            set -a dust_args -d (string replace -- '--max-depth=' '' $arg)
        else if test "$arg" = "-a"; or test "$arg" = "--all"
            # du -a shows all files, not just dirs
            # dust doesn't have an exact equivalent; closest is showing files too
            # no-op — dust shows files by default in its tree
        else if test "$arg" = "-c"; or test "$arg" = "--total"
            # dust shows totals by default, no-op
        else if test "$arg" = "-b"; or test "$arg" = "--bytes"
            set -a dust_args -s -o b
        else if test "$arg" = "--apparent-size"
            set -a dust_args -s
        else if test "$arg" = "-L"; or test "$arg" = "--dereference"
            set -a dust_args -L
        else if string match -qr '^--exclude=' -- $arg
            set -a dust_args -X (string replace -- '--exclude=' '' $arg)
        else if test "$arg" = "--exclude"
            set i (math $i + 1)
            set -a dust_args -X $argv[$i]
        else if test "$arg" = "-x"; or test "$arg" = "--one-file-system"
            set -a dust_args -x
        else if test "$arg" = "--si"
            set -a dust_args -o si
        else if test "$arg" = "-k"
            set -a dust_args -o k
        else if test "$arg" = "-m"
            set -a dust_args -o m
        else if test "$arg" = "-t"; or test "$arg" = "--threshold"
            set i (math $i + 1)
            set -a dust_args -z $argv[$i]
        else if string match -qr '^--threshold=' -- $arg
            set -a dust_args -z (string replace -- '--threshold=' '' $arg)
        else
            if test -e "$arg"
                set -a paths $arg
            else
                set -a dust_args $arg
            end
        end

        set i (math $i + 1)
    end

    if test (count $paths) -gt 0
        command dust $dust_args $paths
    else
        command dust $dust_args
    end
end
