# reef — bash compatibility layer for fish shell
# This file is loaded by fish on startup via conf.d

if not set -q reef_enabled
    set -g reef_enabled true
end

# History mode: bash (default) = store original bash in history
#               fish = store translated fish in history
#               both = store both versions
if not set -q reef_history_mode
    set -g reef_history_mode bash
end

# Display mode: bash (default) = keep original bash visible in terminal
#               fish = show translated fish on commandline (old behavior)
if not set -q reef_display
    set -g reef_display bash
end

# Persistence mode: off (default) = fresh bash each T3 invocation
#                   state = exported vars persist via state file
#                   full  = persistent bash coprocess (everything persists)
if not set -q reef_persist_mode
    set -g reef_persist_mode off
end

# Confirm mode: off (default) = execute immediately
#               on = show translation/passthrough and prompt before executing
if not set -q reef_confirm
    set -g reef_confirm false
end

# --- History Gate (fish 4.0+) ---
# Called by fish before adding any command to history.
# Return 0 = add, return 1 = skip.
function fish_should_add_to_history
    # If reef flagged this command to be skipped, don't record it
    if set -q __reef_skip_history
        set -e __reef_skip_history
        return 1
    end
    # Preserve default fish behavior: skip commands starting with space
    string match -qr '^\s' -- $argv[1]; and return 1
    return 0
end

# --- Enter Key Hook ---
# Chain to any previously-bound Enter handler (oh-my-posh, starship, etc.)
# so their features (transient prompt, etc.) still work alongside reef.
function __reef_chain_enter
    if set -q __reef_prev_enter_handler; and functions -q $__reef_prev_enter_handler
        $__reef_prev_enter_handler
    else
        commandline -f execute
    end
end

# --- Display/History Helpers ---
# Sets up display overwrite and history tracking for bash commands.
function __reef_flag_bash_cmd --argument-names cmd
    if test "$reef_display" = bash
        set -g __reef_display_original $cmd
        set -g __reef_display_prompt (fish_prompt 2>/dev/null | string split \n)[-1]
    end
    set -g __reef_bash_original $cmd
    set -g __reef_skip_history true
end

# --- Build T3 Fallback Command ---
# Returns the appropriate bash-exec or daemon command based on persist mode.
function __reef_build_fallback --argument-names safe_cmd
    switch $reef_persist_mode
        case state
            if set -q __reef_state_file
                echo "reef bash-exec --state-file $__reef_state_file -- '$safe_cmd' | source"
            else
                echo "reef bash-exec -- '$safe_cmd' | source"
            end
        case full
            if set -q __reef_daemon_socket
                echo "__reef_daemon_or_fish $__reef_daemon_socket '$safe_cmd'"
            else
                echo "reef bash-exec -- '$safe_cmd' | source"
            end
        case '*'
            echo "reef bash-exec -- '$safe_cmd' | source"
    end
end

# --- Daemon with fish fallback ---
# Try running through the bash daemon. If bash returns 127 (command not
# found), fall back to running it natively in fish.
function __reef_daemon_or_fish
    set -l socket $argv[1]
    set -l cmd $argv[2..]

    reef daemon exec --socket $socket -- $cmd | source
    set -l daemon_exit $pipestatus[1]

    if test $daemon_exit -eq 127
        eval $cmd
    end
end

# --- Confirm Prompt ---
# Shows what reef will do and asks for Y/n confirmation.
# Temporarily rebinds Enter to default during `read` to prevent re-triggering
# __reef_execute (known fish issue: custom \r bindings intercept read input).
function __reef_confirm_prompt --argument-names type display_str
    if test "$reef_confirm" != true
        return 0
    end
    set -l arrow (set_color cyan)"  →"(set_color normal)
    switch $type
        case translate
            echo $arrow" fish: $display_str"
        case passthrough
            echo $arrow" bash passthrough (no fish equivalent)"
        case daemon
            echo $arrow" bash (persistent coprocess)"
        case source
            echo $arrow" bash source (env capture)"
    end

    # Temporarily rebind Enter to default so `read` can receive it
    bind \r execute
    bind \n execute
    bind -M insert \r execute 2>/dev/null
    bind -M insert \n execute 2>/dev/null

    read -l -P "  Execute? [Y/n] " answer
    set -l read_status $status

    # Restore reef's Enter binding
    bind \r __reef_execute
    bind \n __reef_execute
    bind -M insert \r __reef_execute 2>/dev/null
    bind -M insert \n __reef_execute 2>/dev/null

    if test $read_status -ne 0
        commandline -f repaint
        return 1
    end

    commandline -f repaint
    switch $answer
        case '' y Y yes Yes YES
            return 0
        case '*'
            return 1
    end
end

function __reef_execute
    set -l cmd (commandline)

    if test -z "$cmd"
        __reef_chain_enter
        return
    end

    if test "$reef_enabled" != true; or not command -q reef
        __reef_chain_enter
        return
    end

    # Intercept `source <file>` / `. <file>` for bash scripts.
    # Can't use a source.fish function wrapper — it changes variable scope
    # for all sourced files, breaking conf.d variable definitions.
    if string match -qr '^(source|\.)\s+' -- $cmd
        set -l source_file (string replace -r '^(source|\.)\s+' '' -- $cmd | string trim)
        set source_file (string trim -c '"' -- $source_file | string trim -c "'")
        set source_file (string replace -r '^~' "$HOME" -- $source_file)
        if test -f "$source_file"; and not string match -qr '\.fish$' -- $source_file
            # Non-.fish file exists — route through bash for env capture
            if not __reef_confirm_prompt source
                return
            end
            set -l safe_cmd (string replace -a "'" "'\\''" -- "$cmd")
            commandline -r -- "reef bash-exec --env-diff -- '$safe_cmd' | source"
            __reef_flag_bash_cmd $cmd
            __reef_chain_enter
            return
        end
    end

    # In persist full mode, route bash-detected syntax and unknown commands
    # through the coprocess. Known commands (builtins, functions, PATH binaries)
    # stay in fish — they work the same and keep reef-tools wrappers alive.
    if test "$reef_persist_mode" = full; and set -q __reef_daemon_socket
        set -l first_word (string split -m1 ' ' -- $cmd)[1]
        if reef detect --quick -- "$cmd" 2>/dev/null; or not type -q $first_word
            if not __reef_confirm_prompt daemon
                return
            end
            set -l safe_cmd (string replace -a "'" "'\\''" -- "$cmd")
            commandline -r -- "__reef_daemon_or_fish $__reef_daemon_socket '$safe_cmd'"
            __reef_flag_bash_cmd $cmd
            __reef_chain_enter
            return
        end
    end

    if reef detect --quick -- "$cmd" 2>/dev/null
        set -l translated (reef translate -- "$cmd" 2>/dev/null)
        set -l translate_status $status
        if test $translate_status -eq 0; and test -n "$translated"
            set -l oneliner (string join "; " -- $translated)

            if not __reef_confirm_prompt translate $oneliner
                return
            end

            # In bash display mode: flag for preexec to overwrite the
            # displayed fish translation with the original bash
            if test "$reef_display" = bash
                set -g __reef_display_original $cmd
                set -g __reef_display_prompt (fish_prompt 2>/dev/null | string split \n)[-1]
            end

            if test "$reef_history_mode" != fish
                set -g __reef_bash_original $cmd
                if test "$reef_history_mode" = both
                    set -g __reef_fish_translation $oneliner
                end
                set -g __reef_skip_history true
            end

            commandline -r -- $oneliner
            __reef_chain_enter
            return
        end

        # Translation failed — fall back to bash passthrough.
        if not __reef_confirm_prompt passthrough
            return
        end
        set -l safe_cmd (string replace -a "'" "'\\''" -- "$cmd")
        set -l fallback (__reef_build_fallback $safe_cmd)

        __reef_flag_bash_cmd $cmd
        commandline -r -- $fallback
        __reef_chain_enter
        return
    end

    __reef_chain_enter
end

# --- Display Fixup (runs before command output) ---
# In "bash" display mode, the commandline shows the fish translation.
# This handler fires before any output and overwrites the displayed
# command text with the original bash using ANSI escape codes.
function __reef_fix_display --on-event fish_preexec
    if not set -q __reef_display_original
        return
    end

    set -l orig $__reef_display_original
    set -e __reef_display_original

    set -l prompt_text ""
    if set -q __reef_display_prompt
        set prompt_text $__reef_display_prompt
        set -e __reef_display_prompt
    end

    # The line above cursor shows: [prompt_last_line][fish_cmd]
    # Clear the entire line and rewrite as: [prompt_last_line][original_bash]
    printf '\e7'               # save cursor position
    printf '\e[A\r\e[2K'      # up 1 line, column 0, clear entire line
    printf '%s%s' "$prompt_text" "$orig"
    printf '\e8'               # restore cursor position
end

# --- History Restoration (runs after command completes) ---
# NOTE: Must use "builtin history" — CachyOS overrides the history function,
# which breaks append/delete/merge. "builtin" bypasses the override.
function __reef_restore_history --on-event fish_postexec
    if not set -q __reef_bash_original
        return
    end

    set -l bash_cmd $__reef_bash_original
    set -l fish_cmd $argv[1]
    set -e __reef_bash_original

    switch $reef_history_mode
        case bash
            # Remove the translated fish command from recall buffer + disk
            builtin history delete --exact --case-sensitive -- $fish_cmd
            builtin history save
            # Add the original bash
            builtin history append -- $bash_cmd
        case fish
            # For T2 fallback (no fish translation), store original bash
            # rather than the ugly reef bash-exec wrapper
            builtin history delete --exact --case-sensitive -- $fish_cmd
            builtin history save
            builtin history append -- $bash_cmd
        case both
            builtin history append -- $bash_cmd
            if set -q __reef_fish_translation
                builtin history append -- $__reef_fish_translation
                set -e __reef_fish_translation
            end
    end
end

# --- Cleanup on fish exit ---
function __reef_cleanup --on-event fish_exit
    if set -q __reef_state_file; and test -f "$__reef_state_file"
        rm -f "$__reef_state_file"
    end
    if set -q __reef_daemon_socket
        command reef daemon stop --socket $__reef_daemon_socket 2>/dev/null
    end
end

# --- Toggle / Settings ---
function reef --description "reef: bash compatibility settings"
    # Check for --silent flag and strip it from argv
    set -l _silent false
    set -l _args
    for _a in $argv
        if test "$_a" = --silent
            set _silent true
        else
            set -a _args $_a
        end
    end
    set argv $_args

    switch "$argv[1]"
        case on enable
            set -g reef_enabled true
            test "$_silent" != true; and echo "reef: translation enabled"
        case off disable
            set -g reef_enabled false
            test "$_silent" != true; and echo "reef: translation disabled"
        case status ''
            set -l confirm_label (test "$reef_confirm" = true; and echo on; or echo off)
            if test "$reef_enabled" = true
                echo "reef: enabled (display: $reef_display, history: $reef_history_mode, persist: $reef_persist_mode, confirm: $confirm_label)"
            else
                echo "reef: disabled (display: $reef_display, history: $reef_history_mode, persist: $reef_persist_mode, confirm: $confirm_label)"
            end
        case display
            switch "$argv[2]"
                case bash
                    set -g reef_display bash
                    test "$_silent" != true; and echo "reef: display → original bash commands"
                case fish
                    set -g reef_display fish
                    test "$_silent" != true; and echo "reef: display → translated fish commands"
                case '' status
                    echo "reef: display mode: $reef_display"
                case '*'
                    echo "reef: unknown display mode '$argv[2]' (use: bash, fish)"
            end
        case history
            switch "$argv[2]"
                case bash
                    set -g reef_history_mode bash
                    test "$_silent" != true; and echo "reef: history → original bash commands"
                case fish
                    set -g reef_history_mode fish
                    test "$_silent" != true; and echo "reef: history → translated fish commands"
                case both
                    set -g reef_history_mode both
                    test "$_silent" != true; and echo "reef: history → both bash and fish commands"
                case '' status
                    echo "reef: history mode: $reef_history_mode"
                case '*'
                    echo "reef: unknown history mode '$argv[2]' (use: bash, fish, both)"
            end
        case persist
            switch "$argv[2]"
                case off
                    # Clean up state file
                    if set -q __reef_state_file; and test -f "$__reef_state_file"
                        rm -f "$__reef_state_file"
                        set -e __reef_state_file
                    end
                    # Stop daemon if running
                    if set -q __reef_daemon_socket
                        command reef daemon stop --socket $__reef_daemon_socket 2>/dev/null
                        set -e __reef_daemon_socket
                    end
                    set -g reef_persist_mode off
                    test "$_silent" != true; and echo "reef: persistence off (fresh bash each time)"
                case state
                    # Stop daemon if switching from full
                    if set -q __reef_daemon_socket
                        command reef daemon stop --socket $__reef_daemon_socket 2>/dev/null
                        set -e __reef_daemon_socket
                    end
                    set -g reef_persist_mode state
                    set -g __reef_state_file /tmp/reef-state-$fish_pid
                    test "$_silent" != true; and echo "reef: persistence → state (exported vars persist across commands)"
                case full
                    # Clean up state file if switching from state
                    if set -q __reef_state_file; and test -f "$__reef_state_file"
                        rm -f "$__reef_state_file"
                        set -e __reef_state_file
                    end
                    set -g reef_persist_mode full
                    set -g __reef_daemon_socket /tmp/reef-$fish_pid.sock
                    command reef daemon start --socket $__reef_daemon_socket
                    test "$_silent" != true; and echo "reef: persistence → full (persistent bash coprocess)"
                case status ''
                    echo "reef: persist mode: $reef_persist_mode"
                case '*'
                    echo "reef: unknown persist mode '$argv[2]' (use: off, state, full)"
            end
        case confirm
            switch "$argv[2]"
                case on
                    set -g reef_confirm true
                    test "$_silent" != true; and echo "reef: confirm → prompt before execution"
                case off
                    set -g reef_confirm false
                    test "$_silent" != true; and echo "reef: confirm → execute immediately"
                case '' status
                    if test "$reef_confirm" = true
                        echo "reef: confirm mode: on"
                    else
                        echo "reef: confirm mode: off"
                    end
                case '*'
                    echo "reef: unknown confirm mode '$argv[2]' (use: on, off)"
            end
        case '*'
            command reef $argv
    end
end

# --- Deferred Binding Setup ---
# Runs on first prompt so we load AFTER other tools (oh-my-posh, starship, etc.)
# that also bind Enter. This lets us capture their handler and chain to it.
function __reef_setup --on-event fish_prompt
    functions -e __reef_setup

    # Save existing Enter handler for chaining (check insert mode first,
    # then default — prompt tools like oh-my-posh bind across all modes)
    set -l prev (bind -M insert \r 2>/dev/null | string match -r '\S+$')
    if not test -n "$prev"; or not functions -q $prev
        set prev (bind \r 2>/dev/null | string match -r '\S+$')
    end
    if test -n "$prev"; and functions -q $prev; and test "$prev" != __reef_execute
        set -g __reef_prev_enter_handler $prev
    end

    bind \r __reef_execute
    bind \n __reef_execute

    if bind -M insert \r >/dev/null 2>&1
        bind -M insert \r __reef_execute
        bind -M insert \n __reef_execute
    end

    # Erase any distro/user aliases that shadow reef-tools wrappers
    # (e.g. CachyOS, Garuda define simple eza/rg aliases).
    # Fish will autoload our smarter versions from vendor_functions.d on first use.
    # Only erase if our wrapper file exists, so uninstalling reef-tools restores originals.
    for __reef_tool in ls cat grep find sed du ps
        for __reef_dir in $fish_function_path
            if string match -q '*vendor_functions.d' -- $__reef_dir
                and test -f "$__reef_dir/$__reef_tool.fish"
                functions -e $__reef_tool
                break
            end
        end
    end
    # Erase sub-aliases that conflict (only if we erased the parent)
    functions -q ls; or functions -e la ll lt
    functions -q grep; or functions -e fgrep egrep

    # Auto-source .bashrc
    if test -f ~/.bashrc; and command -q reef
        reef bash-exec --env-diff -- "source ~/.bashrc" 2>/dev/null | source
    end
end
