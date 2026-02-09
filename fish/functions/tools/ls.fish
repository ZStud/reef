function ls --description "GNU ls → eza wrapper"
    if not command -q eza
        command ls $argv
        return $status
    end

    # Pass through --version/--help to the real tool
    for a in $argv
        if test "$a" = "--version"; or test "$a" = "--help"
            command ls $argv
            return $status
        end
    end

    set -l eza_args
    set -l i 1
    while test $i -le (count $argv)
        set -l arg $argv[$i]

        if test "$arg" = "-a"; or test "$arg" = "--all"
            set -a eza_args -a
        else if test "$arg" = "-A"; or test "$arg" = "--almost-all"
            set -a eza_args -a
        else if test "$arg" = "-l"
            set -a eza_args -l
        else if test "$arg" = "-h"; or test "$arg" = "--human-readable"
            # eza default, no-op
        else if test "$arg" = "-R"; or test "$arg" = "--recursive"
            set -a eza_args --tree
        else if test "$arg" = "-r"; or test "$arg" = "--reverse"
            set -a eza_args --reverse
        else if test "$arg" = "-S"
            set -a eza_args --sort=size
        else if test "$arg" = "-t"
            set -a eza_args --sort=modified
        else if test "$arg" = "-X"
            set -a eza_args --sort=extension
        else if test "$arg" = "-1"
            set -a eza_args --oneline
        else if test "$arg" = "-d"; or test "$arg" = "--directory"
            set -a eza_args -d
        else if test "$arg" = "-F"; or test "$arg" = "--classify"
            set -a eza_args --classify
        else if test "$arg" = "-i"; or test "$arg" = "--inode"
            set -a eza_args --inode
        else if test "$arg" = "-g"
            set -a eza_args -l --no-user
        else if test "$arg" = "-o"
            set -a eza_args -l --no-group
        else if test "$arg" = "--color"
            set i (math $i + 1)
            set -a eza_args --color=$argv[$i]
        else if string match -qr '^--color=' -- $arg
            set -a eza_args $arg
        else if test "$arg" = "--group-directories-first"
            set -a eza_args --group-directories-first
        else if test "$arg" = "--sort"
            set i (math $i + 1)
            set -a eza_args --sort=$argv[$i]
        else if string match -qr '^--sort=' -- $arg
            set -a eza_args $arg
        else if test "$arg" = "-la"; or test "$arg" = "-al"
            set -a eza_args -la
        else if test "$arg" = "-lh"
            set -a eza_args -l
        else if test "$arg" = "-lR"
            set -a eza_args -l --tree
        else if test "$arg" = "-lt"
            set -a eza_args -l --sort=modified
        else if test "$arg" = "-lS"
            set -a eza_args -l --sort=size
        else if test "$arg" = "-ltr"; or test "$arg" = "-lrt"
            set -a eza_args -l --sort=modified --reverse
        else
            set -a eza_args $arg
        end

        set i (math $i + 1)
    end

    command eza $eza_args
end
