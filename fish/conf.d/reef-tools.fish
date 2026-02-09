# reef-tools — load tool wrappers at startup
# conf.d runs after functions/ but can override autoloaded functions.
# This is needed because fish ships its own grep.fish and ls.fish
# in /usr/share/fish/functions/ which shadow vendor_functions.d/.

set -l tool_dir (status dirname)/../functions/tools
if not test -d "$tool_dir"
    # AUR install: wrappers are in vendor_functions.d, already loaded as functions.
    # But fish's own grep.fish/ls.fish still shadow them — re-source from vendor path.
    set tool_dir /usr/share/fish/vendor_functions.d
    for tool in grep find sed du ps ls
        if test -f $tool_dir/$tool.fish
            source $tool_dir/$tool.fish
        end
    end
else
    # From-source install: wrappers are in the tools/ subdirectory
    for tool in grep find sed du ps ls
        if test -f $tool_dir/$tool.fish
            source $tool_dir/$tool.fish
        end
    end
end
