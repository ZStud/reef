function ps --description "GNU ps → procs wrapper"
    if not command -q procs
        command ps $argv
        return $status
    end

    set -l procs_args

    set -l i 1
    while test $i -le (count $argv)
        set -l arg $argv[$i]

        if test "$arg" = "aux"; or test "$arg" = "-aux"
            # ps aux — procs shows all by default, no-op
        else if test "$arg" = "-ef"
            # ps -ef — procs shows all by default, no-op
        else if test "$arg" = "-e"; or test "$arg" = "--every"; or test "$arg" = "-A"
            # all processes — procs default, no-op
        else if test "$arg" = "-f"
            # full format — procs default, no-op
        else if test "$arg" = "-u"
            if test (math $i + 1) -le (count $argv)
                set -l next $argv[(math $i + 1)]
                if not string match -qr '^-' -- $next
                    set i (math $i + 1)
                    set -a procs_args $next
                end
            end
        else if test "$arg" = "-p"; or test "$arg" = "--pid"
            # procs doesn't reliably support PID lookup — fall back to real ps
            command ps $argv
            return $status
        else if string match -qr '^-p[0-9]' -- $arg
            command ps $argv
            return $status
        else if string match -qr '^--sort=' -- $arg
            set -l sortkey (string replace -- '--sort=' '' $arg)
            if test "$sortkey" = "-%cpu"; or test "$sortkey" = "-pcpu"; or test "$sortkey" = "%cpu"; or test "$sortkey" = "pcpu"
                set -a procs_args --sortd cpu
            else if test "$sortkey" = "-%mem"; or test "$sortkey" = "-pmem"; or test "$sortkey" = "%mem"; or test "$sortkey" = "pmem"
                set -a procs_args --sortd mem
            else if test "$sortkey" = "-pid"; or test "$sortkey" = "pid"
                set -a procs_args --sorta pid
            else if test "$sortkey" = "-rss"; or test "$sortkey" = "rss"
                set -a procs_args --sortd mem
            else
                set -a procs_args --sortd $sortkey
            end
        else if test "$arg" = "--sort"
            set i (math $i + 1)
            set -l sortkey $argv[$i]
            if test "$sortkey" = "-%cpu"; or test "$sortkey" = "%cpu"
                set -a procs_args --sortd cpu
            else if test "$sortkey" = "-%mem"; or test "$sortkey" = "%mem"
                set -a procs_args --sortd mem
            else
                set -a procs_args --sortd $sortkey
            end
        else if test "$arg" = "-C"
            set i (math $i + 1)
            set -a procs_args $argv[$i]
        else if test "$arg" = "--no-headers"; or test "$arg" = "--no-heading"
            set -a procs_args --no-header
        else if test "$arg" = "-w"
            # wide output — procs auto-sizes, no-op
        else if test "$arg" = "-o"
            # custom output format — skip, procs has fixed columns
            set i (math $i + 1)
        else
            if not string match -qr '^-' -- $arg
                set -a procs_args $arg
            else
                set -a procs_args $arg
            end
        end

        set i (math $i + 1)
    end

    command procs $procs_args
end
