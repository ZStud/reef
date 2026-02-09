function grep --description "GNU grep → ripgrep (rg) wrapper"
    if not command -q rg
        command grep $argv
        return $status
    end

    # Pass through --version/--help to the real tool
    for a in $argv
        if test "$a" = "--version"; or test "$a" = "--help"
            command grep $argv
            return $status
        end
    end

    set -l rg_args

    set -l i 1
    while test $i -le (count $argv)
        set -l arg $argv[$i]

        if test "$arg" = "-E"; or test "$arg" = "--extended-regexp"
            # ERE — rg default, drop
        else if test "$arg" = "-G"; or test "$arg" = "--basic-regexp"
            # BRE — rg doesn't have this, drop silently
        else if test "$arg" = "-P"; or test "$arg" = "--perl-regexp"
            set -a rg_args -P
        else if test "$arg" = "-F"; or test "$arg" = "--fixed-strings"
            set -a rg_args -F
        else if test "$arg" = "-i"; or test "$arg" = "--ignore-case"
            set -a rg_args -i
        else if test "$arg" = "-v"; or test "$arg" = "--invert-match"
            set -a rg_args -v
        else if test "$arg" = "-w"; or test "$arg" = "--word-regexp"
            set -a rg_args -w
        else if test "$arg" = "-x"; or test "$arg" = "--line-regexp"
            set -a rg_args -x
        else if test "$arg" = "-c"; or test "$arg" = "--count"
            set -a rg_args -c
        else if test "$arg" = "-l"; or test "$arg" = "--files-with-matches"
            set -a rg_args -l
        else if test "$arg" = "-L"; or test "$arg" = "--files-without-match"
            set -a rg_args --files-without-match
        else if test "$arg" = "-n"; or test "$arg" = "--line-number"
            set -a rg_args -n
        else if test "$arg" = "-H"; or test "$arg" = "--with-filename"
            # rg default, no-op
        else if test "$arg" = "-h"; or test "$arg" = "--no-filename"
            set -a rg_args --no-filename
        else if test "$arg" = "-o"; or test "$arg" = "--only-matching"
            set -a rg_args -o
        else if test "$arg" = "-q"; or test "$arg" = "--quiet"; or test "$arg" = "--silent"
            set -a rg_args -q
        else if test "$arg" = "-s"; or test "$arg" = "--no-messages"
            set -a rg_args --no-messages
        else if test "$arg" = "-A"
            set i (math $i + 1)
            set -a rg_args -A $argv[$i]
        else if test "$arg" = "-B"
            set i (math $i + 1)
            set -a rg_args -B $argv[$i]
        else if test "$arg" = "-C"
            set i (math $i + 1)
            set -a rg_args -C $argv[$i]
        else if test "$arg" = "-m"; or test "$arg" = "--max-count"
            set i (math $i + 1)
            set -a rg_args -m $argv[$i]
        else if string match -qr '^--max-count=' -- $arg
            set -a rg_args --max-count=(string replace -- '--max-count=' '' $arg)
        else if test "$arg" = "-e"
            set i (math $i + 1)
            set -a rg_args -e $argv[$i]
        else if test "$arg" = "-f"
            set i (math $i + 1)
            set -a rg_args -f $argv[$i]
        else if test "$arg" = "-r"; or test "$arg" = "-R"; or test "$arg" = "--recursive"
            # rg is recursive by default, no-op
        else if string match -qr '^--include=' -- $arg
            set -l glob (string replace -- '--include=' '' $arg)
            set -a rg_args -g $glob
        else if test "$arg" = "--include"
            set i (math $i + 1)
            set -a rg_args -g $argv[$i]
        else if string match -qr '^--exclude=' -- $arg
            set -l glob (string replace -- '--exclude=' '' $arg)
            set -a rg_args -g "!$glob"
        else if test "$arg" = "--exclude"
            set i (math $i + 1)
            set -a rg_args -g "!$argv[$i]"
        else if string match -qr '^--exclude-dir=' -- $arg
            set -l dir (string replace -- '--exclude-dir=' '' $arg)
            set -a rg_args -g "!$dir/"
        else if test "$arg" = "--exclude-dir"
            set i (math $i + 1)
            set -a rg_args -g "!$argv[$i]/"
        else if string match -qr '^--color=' -- $arg
            set -a rg_args $arg
        else if test "$arg" = "--color"
            set i (math $i + 1)
            set -a rg_args --color=$argv[$i]
        else
            # Pass through (patterns, filenames, rg-native flags)
            set -a rg_args $arg
        end

        set i (math $i + 1)
    end

    command rg $rg_args
end
