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

    if reef detect --quick -- "$cmd" 2>/dev/null
        set -l translated (reef translate -- "$cmd" 2>/dev/null)
        set -l translate_status $status
        if test $translate_status -eq 0; and test -n "$translated"
            set -l oneliner (string join "; " -- $translated)

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
        set -l safe_cmd (string replace -a "'" "'\\''" -- "$cmd")
        set -l fallback "reef bash-exec -- '$safe_cmd' | source"

        # In bash display mode: flag for preexec overwrite
        if test "$reef_display" = bash
            set -g __reef_display_original $cmd
            set -g __reef_display_prompt (fish_prompt 2>/dev/null | string split \n)[-1]
        end

        # Always skip the ugly fallback from history and store original bash
        set -g __reef_bash_original $cmd
        set -g __reef_skip_history true

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

# --- Toggle / Settings ---
function reef --description "reef: bash compatibility settings"
    switch "$argv[1]"
        case on enable
            set -g reef_enabled true
            echo "reef: translation enabled"
        case off disable
            set -g reef_enabled false
            echo "reef: translation disabled"
        case status ''
            if test "$reef_enabled" = true
                echo "reef: enabled (display: $reef_display, history: $reef_history_mode)"
            else
                echo "reef: disabled (display: $reef_display, history: $reef_history_mode)"
            end
        case display
            switch "$argv[2]"
                case bash
                    set -g reef_display bash
                    echo "reef: display → original bash commands"
                case fish
                    set -g reef_display fish
                    echo "reef: display → translated fish commands"
                case '' status
                    echo "reef: display mode: $reef_display"
                case '*'
                    echo "reef: unknown display mode '$argv[2]' (use: bash, fish)"
            end
        case history
            switch "$argv[2]"
                case bash
                    set -g reef_history_mode bash
                    echo "reef: history → original bash commands"
                case fish
                    set -g reef_history_mode fish
                    echo "reef: history → translated fish commands"
                case both
                    set -g reef_history_mode both
                    echo "reef: history → both bash and fish commands"
                case '' status
                    echo "reef: history mode: $reef_history_mode"
                case '*'
                    echo "reef: unknown history mode '$argv[2]' (use: bash, fish, both)"
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
    if test -n "$prev"; and functions -q $prev
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
