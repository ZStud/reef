#!/usr/bin/env fish
# reef installer — copies fish components to the right places
#
# Usage:
#   fish install.fish              Install core functions + conf.d
#   fish install.fish --tools      Also install tool wrappers (grep→rg, find→fd, etc.)
#   fish install.fish --all        Install everything
#   fish install.fish --uninstall  Remove all reef-installed files

set -l fish_functions ~/.config/fish/functions
set -l fish_confd ~/.config/fish/conf.d
set -l script_dir (status dirname)
set -l install_tools false
set -l uninstall false

# Parse flags
for arg in $argv
    switch $arg
        case --tools --all
            set install_tools true
        case --uninstall --remove
            set uninstall true
    end
end

# Core reef files
set -l core_functions \
    export.fish \
    unset.fish \
    declare.fish \
    local.fish \
    readonly.fish \
    shopt.fish \
    source.fish \
    fish_command_not_found.fish

# Tool wrapper files
set -l tool_functions \
    grep.fish \
    find.fish \
    sed.fish \
    du.fish \
    ps.fish \
    ls.fish

if test $uninstall = true
    echo "reef: uninstalling..."
    for f in $core_functions
        if test -f $fish_functions/$f
            rm $fish_functions/$f
            echo "  removed $f"
        end
    end
    for f in $tool_functions
        if test -f $fish_functions/$f
            rm $fish_functions/$f
            echo "  removed $f"
        end
    end
    if test -f $fish_confd/reef.fish
        rm $fish_confd/reef.fish
        echo "  removed conf.d/reef.fish"
    end
    if test -f $fish_confd/reef-tools.fish
        rm $fish_confd/reef-tools.fish
        echo "  removed conf.d/reef-tools.fish"
    end
    echo ""
    echo "reef: uninstalled. Restart fish or run 'exec fish' to deactivate."
    return 0
end

echo "reef: installing fish components..."

# Ensure directories exist
mkdir -p $fish_functions $fish_confd

# Install core function files
for f in $core_functions
    if test -f $script_dir/functions/$f
        cp $script_dir/functions/$f $fish_functions/$f
        echo "  installed $f"
    else
        echo "  warning: $f not found" >&2
    end
end

# Install conf.d
for f in $script_dir/conf.d/*.fish
    set -l name (basename $f)
    cp $f $fish_confd/$name
    echo "  installed conf.d/$name"
end

# Tool wrappers (optional)
if test $install_tools = true
    echo ""
    echo "reef: installing tool wrappers..."
    for f in $tool_functions
        if test -f $script_dir/functions/tools/$f
            if test -f $fish_functions/$f
                echo "  overwriting $f (existing backed up to $f.bak)"
                cp $fish_functions/$f $fish_functions/$f.bak
            end
            cp $script_dir/functions/tools/$f $fish_functions/$f
            echo "  installed $f (tool wrapper)"
        end
    end
end

echo ""
echo "reef: fish components installed."

# Check for reef binary
if not command -q reef
    echo ""
    echo "reef: binary not found in PATH."
    echo "  Build and install it with:"
    echo "    cargo install --path ."
    echo "  Or copy target/release/reef to somewhere in your PATH."
end

echo ""
echo "reef: restart fish or run 'exec fish' to activate."
