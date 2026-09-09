# The settings model: the schema the terminal states on one side, the
# user's config file on the other, and the resolution rule from
# docs/config.md between them. A key's value is the file's if the file
# pins it, otherwise the value the table's named preset gives it,
# otherwise the shipped default.
#
# The schema - defaults, presets, enum value lists, the bundled font
# catalogue - is asked of the terminal when the window opens: `init` runs
# `robco-term --dump-settings` and reads what it prints, so the terminal's
# Rust source is the only place these values are stated and this window
# cannot hold a copy of them to go stale. The window therefore needs the
# binary present to open, found the way `candidates` below walks. The
# machine's installed faces are the same seam asked later, on the font
# tab, because that answer costs a walk of the platform's font
# directories.
#
# Every write here goes through the machine-write contract
# (docs/config-format.md): re-read the file, edit the one key's bytes,
# atomic_write. The re-read is what keeps the read-modify-write window
# short; there is no lock, and last writer wins.

package require Tcl 9.0

source [file join [file dirname [info script]] tomledit-1.1.tm]

namespace eval ::rcsettings::model {
    namespace export init load path set_path text reload \
        effective effective_raw base base_raw pinned preset_name \
        set_value reset switch_preset pin_overrides default_config_path \
        ssh_default ssh_hosts set_ssh_default add_ssh_host remove_ssh_host \
        set_ssh_host \
        load_schema shipped has_default table_keys preset_names has_preset \
        preset enum enum_names fonts system_fonts system_fonts_text

    # The whole model is one document: the GUI edits a single config
    # file, so there is nothing to instantiate.
    variable Path {}
    variable Text ""

    # Tables whose `name` key selects a preset base rather than labelling
    # the table. The axis name and the table name are the same word.
    variable Axes {screen chassis}

    # The ssh rows are an array of tables, `[[ssh.host]]`, and each row is
    # a diff against `[ssh_host_defaults]` the way a flat table is a diff
    # against its preset. These two names are written out often enough
    # below to be worth naming once.
    variable SshRows ssh.host
    variable SshFields {host user port key}

    # ------------------------------------------------------- the schema --
    #
    # What `--dump-settings` prints, in the shapes it prints it. Values are
    # raw TOML text, quoting intact, so an edited value can be formatted
    # after its default's own spelling. `[ssh_host_defaults]` is not a
    # table of the config file: it is what one `[[ssh.host]]` row's fields
    # fall back to. Presets arrive with every field resolved, one entry per
    # preset in the dump's own order.

    variable Defaults [dict create]
    variable Presets [dict create]
    variable Values [dict create]
    variable Fonts {}

    # The dump is one ask per process: what the binary states cannot move
    # under a window that is already open.
    variable SchemaLoaded 0

    # The ask itself. `init` runs it, and so does any caller that reads the
    # schema without a config file to open.
    proc load_schema {} {
        variable SchemaLoaded
        if {$SchemaLoaded} { return }
        read_schema [run_dump [locate] --dump-settings]
        set SchemaLoaded 1
        return
    }

    # The parse step alone, so a caller can feed a dump it already holds.
    proc read_schema {text} {
        variable Defaults
        variable Presets
        variable Values
        variable Fonts
        set parsed [::tomledit::parse $text]
        set Defaults [dict create]
        foreach table {general screen chassis ssh ssh_host_defaults critters serial} {
            dict set Defaults $table [dict get $parsed tables $table]
        }
        set Presets [dict create]
        foreach {axis rows} {screen screen_presets chassis chassis_presets} {
            dict set Presets $axis [dict create]
            foreach entry [dict get $parsed arrays $rows] {
                dict set Presets $axis \
                    [::tomledit::plain [dict get $entry name]] $entry
            }
        }
        set Values [dict create]
        dict for {name raw} [dict get $parsed tables values] {
            dict set Values $name [string_array $raw]
        }
        set Fonts [font_pairs [dict get $parsed arrays fonts]]
        return
    }

    # The elements of a TOML array of strings, whose raw text the parser
    # has already joined onto one line. Only string arrays appear in
    # `[values]`.
    proc string_array {raw} {
        set out {}
        foreach {- item} [regexp -all -inline {"((?:[^"\\]|\\.)*)"} $raw] {
            lappend out [::tomledit::plain "\"$item\""]
        }
        return $out
    }

    # {catalogue_key display_name} pairs from parsed `[[fonts]]` rows. The
    # bundled catalogue and the machine's own faces are the same shape.
    proc font_pairs {entries} {
        return [lmap entry $entries {
            list [::tomledit::plain [dict get $entry name]] \
                [::tomledit::plain [dict get $entry text]]
        }]
    }

    # The shipped default of table.key, raw. Asking for a key the schema
    # does not carry is a caller's bug and errors by name.
    proc shipped {table key} {
        variable Defaults
        if {![dict exists $Defaults $table $key]} {
            error "no default for $table.$key in the schema"
        }
        return [dict get $Defaults $table $key]
    }

    proc has_default {table key} {
        variable Defaults
        return [dict exists $Defaults $table $key]
    }

    # Declaration order, which is the order a preset picker should offer.
    proc table_keys {table} {
        variable Defaults
        if {![dict exists $Defaults $table]} {
            error "no table \"$table\" in the schema"
        }
        return [dict keys [dict get $Defaults $table]]
    }

    # Dump order, which is the order a preset picker should offer.
    proc preset_names {axis} {
        variable Presets
        if {![dict exists $Presets $axis]} {
            error "no presets for axis \"$axis\""
        }
        return [dict keys [dict get $Presets $axis]]
    }

    proc has_preset {axis name} {
        variable Presets
        return [dict exists $Presets $axis $name]
    }

    # The preset with every field of its table resolved, `name` among
    # them, which is how the dump states it.
    proc preset {axis name} {
        variable Presets
        if {![has_preset $axis $name]} {
            error "no built-in $axis preset named \"$name\""
        }
        return [dict get $Presets $axis $name]
    }

    # The bundled catalogue: {catalogue_key display_name} pairs.
    proc fonts {} {
        variable Fonts
        return $Fonts
    }

    proc enum_names {} {
        variable Values
        return [dict keys $Values]
    }

    proc enum {name} {
        variable Values
        if {![dict exists $Values $name]} {
            error "no value list named \"$name\""
        }
        return [dict get $Values $name]
    }

    # ------------------------------------------- the machine's own faces --
    #
    # The installed system faces are asked of the same binary the schema
    # is, under a flag of their own, because that answer costs a walk of
    # the platform's font directories: the font tab asks for it, and a
    # window the user never takes there does not pay for it. The terminal
    # names itself in ROBCO_SETTINGS_TERMINAL when it opens this window; a
    # hand launch falls back to the sibling binary and then PATH.

    # A binary the suites point this namespace at, so a test can run the
    # real dump and font walk against a build tree. Nothing user-facing
    # sets it.
    variable ForcedBinary ""

    # Windows resolves an executable by extension, so the sibling looked
    # for there is robco-term.exe; nowhere else carries a suffix.
    proc exe_suffix {} {
        return [expr {$::tcl_platform(platform) eq "windows" ? ".exe" : ""}]
    }

    proc candidates {} {
        variable ForcedBinary
        global env
        set out {}
        if {$ForcedBinary ne ""} { lappend out $ForcedBinary }
        if {[info exists ::rcsettings::embedded(terminal)]
            && $::rcsettings::embedded(terminal) ne ""} {
            lappend out $::rcsettings::embedded(terminal)
        }
        if {[info exists env(ROBCO_SETTINGS_TERMINAL)]
            && $env(ROBCO_SETTINGS_TERMINAL) ne ""} {
            lappend out $env(ROBCO_SETTINGS_TERMINAL)
        }
        set exe [info nameofexecutable]
        if {$exe ne ""} {
            lappend out [file join [file dirname $exe] robco-term[exe_suffix]]
        }
        set found [auto_execok robco-term]
        if {$found ne ""} { lappend out [lindex $found 0] }
        return $out
    }

    proc locate {} {
        foreach path [candidates] {
            if {[file executable $path] && ![file isdirectory $path]} {
                return $path
            }
        }
        error [describe_search]
    }

    # The message locate fails with: each arm it names is an arm
    # candidates actually walks, and the tests hold the two together.
    proc describe_search {} {
        return "cannot find the robco-term binary: not named in\
            ROBCO_SETTINGS_TERMINAL, not beside this program\
            (robco-term[exe_suffix]), not on PATH"
    }

    # Run the binary under one flag and hand back its raw stdout. The
    # binary logs to stderr and a log line is not a failure, so stderr
    # goes to a file; only a non-zero exit fails, and then what the
    # binary said is the useful half of the message.
    proc run_dump {path flag} {
        set ch [file tempfile errpath]
        close $ch
        set failed [catch {exec -- $path $flag 2> $errpath} out]
        set said [string trim [::tomledit::read_file $errpath]]
        file delete -- $errpath
        if {$failed} {
            error "running \"$path $flag\" failed: $out\
                [expr {$said eq "" ? "" : "\n$said"}]"
        }
        return $out
    }

    # The installed system faces. An empty machine is a legal answer, not
    # a broken one.
    proc system_fonts {{path ""}} {
        if {$path eq ""} { set path [locate] }
        return [system_fonts_text [run_dump $path --list-renderable-fonts]]
    }

    # The parse step alone, so tests can feed a canned dump. {name text}
    # pairs, same shape as `fonts`, and an empty list when the array is
    # absent altogether.
    proc system_fonts_text {text} {
        set parsed [::tomledit::parse $text]
        if {![dict exists $parsed arrays fonts]} { return {} }
        return [font_pairs [dict get $parsed arrays fonts]]
    }

    # ------------------------------------------------- the config file --

    proc default_config_path {} {
        global env tcl_platform
        if {$tcl_platform(platform) eq "windows"} {
            if {![info exists env(APPDATA)]} {
                error "APPDATA is not set; cannot locate the config file"
            }
            return [file join $env(APPDATA) robco-term config.toml]
        }
        if {$tcl_platform(os) eq "Darwin"} {
            return [file join $env(HOME) Library "Application Support" \
                robco-term config.toml]
        }
        if {[info exists env(XDG_CONFIG_HOME)] && $env(XDG_CONFIG_HOME) ne ""} {
            return [file join $env(XDG_CONFIG_HOME) robco-term config.toml]
        }
        return [file join $env(HOME) .config robco-term config.toml]
    }

    # An empty $path takes the platform's default location. The schema
    # comes first: without it a window has nothing to resolve the file
    # against, so the binary being absent is a failure to open, not a
    # window with empty pages.
    proc init {{path ""}} {
        variable Path
        load_schema
        if {$path eq ""} { set path [default_config_path] }
        set Path $path
        reload
        return
    }

    proc load {path} {
        variable Path
        set Path $path
        reload
    }

    proc path {} {
        variable Path
        return $Path
    }

    proc set_path {path} {
        variable Path
        set Path $path
        reload
    }

    proc text {} {
        variable Text
        return $Text
    }

    # A missing file is the empty document, per the contract, so this
    # never fails on a fresh install.
    proc reload {} {
        variable Path
        variable Text
        set Text [::tomledit::read_file $Path]
        return $Text
    }

    # What every mutating operation starts from: the file as it stands,
    # refused whole when it carries TOML outside the surgeon's subset
    # (tomledit's unsafe). Reading stays open to any file; only editing
    # is gated, and the refusal names the line so the user can hand-edit.
    proc edit_base {} {
        set text [reload]
        set what [::tomledit::unsafe $text]
        if {$what ne ""} {
            error "this file uses TOML the settings window cannot edit safely ($what); edit it by hand instead"
        }
        return $text
    }

    proc parsed_tables {text} {
        return [dict get [::tomledit::parse $text] tables]
    }

    proc raw_in {text table key} {
        set tables [parsed_tables $text]
        if {[dict exists $tables $table $key]} {
            return [dict get $tables $table $key]
        }
        return {}
    }

    proc pinned {table key} {
        variable Text
        set tables [parsed_tables $Text]
        return [dict exists $tables $table $key]
    }

    # Which preset the table currently resolves against: the file's `name`
    # if it pins one, else the shipped table's own.
    proc preset_name {table} {
        variable Text
        variable Axes
        if {$table ni $Axes} { return "" }
        set tables [parsed_tables $Text]
        if {[dict exists $tables $table name]} {
            return [::tomledit::plain [dict get $tables $table name]]
        }
        return [::tomledit::plain [shipped $table name]]
    }

    # The raw value a key falls back to when the file does not pin it,
    # given a preset to measure against. A name matching no built-in
    # preset falls back to the shipped default, which is what the loader
    # does with a look the user saved under a name of their own.
    proc base_raw_under {table presetname key} {
        variable Axes
        if {$table in $Axes && $key ne "name"} {
            if {[has_preset $table $presetname]} {
                set entry [preset $table $presetname]
                if {[dict exists $entry $key]} {
                    return [dict get $entry $key]
                }
            }
        }
        return [shipped $table $key]
    }

    proc base_raw {table key} {
        return [base_raw_under $table [preset_name $table] $key]
    }

    proc base {table key} {
        return [::tomledit::plain [base_raw $table $key]]
    }

    proc effective_raw {table key} {
        variable Text
        set tables [parsed_tables $Text]
        if {[dict exists $tables $table $key]} {
            return [dict get $tables $table $key]
        }
        return [base_raw $table $key]
    }

    proc effective {table key} {
        return [::tomledit::plain [effective_raw $table $key]]
    }

    # Two raw values meaning the same setting. Numbers are compared as
    # numbers because the file's spelling of one ("0.90", "1") is the
    # user's and need not match the schema's.
    proc same_value {type a b} {
        if {$type in {int float}} {
            if {[string is double -strict $a] && [string is double -strict $b]} {
                return [expr {double($a) == double($b)}]
            }
        }
        return [string equal $a $b]
    }

    # The minimal edit for setting a key: when the new value is what the
    # key would fall back to anyway, the edit is to remove the key, not to
    # write the value out. $value is a plain Tcl value; it is formatted
    # after the type of the shipped default, which is the only place the
    # key's type is stated.
    proc set_value {table key value} {
        set type [::tomledit::type_of [shipped $table $key]]
        set raw [::tomledit::format_value $type $value]
        set text [edit_base]
        if {[same_value $type $raw [base_raw $table $key]]} {
            set text [::tomledit::del $text $table.$key]
        } else {
            set text [::tomledit::put $text $table.$key $raw]
        }
        return [commit $text]
    }

    # Unpin a key, letting it fall back to its base. Note that its base is
    # the named preset's value, not necessarily the shipped default.
    proc reset {table key} {
        set text [::tomledit::del [edit_base] $table.$key]
        return [commit $text]
    }

    # Switching presets, not renaming a look: the new preset's values are
    # what the user asked to see, so the old table's overrides go. Only
    # keys the schema knows are dropped; a key this tool does not
    # recognise belongs to someone else and is round-tripped.
    proc switch_preset {table presetname} {
        set text [edit_base]
        foreach key [table_keys $table] {
            if {$key eq "name"} { continue }
            set text [::tomledit::del $text $table.$key]
        }
        return [commit [write_name $text $table $presetname]]
    }

    # Renaming a look: the visible values are what the user asked to keep,
    # so every key whose value would move under the new base gets pinned
    # at what it shows now. A key already pinned at what the new base
    # gives it is unpinned instead, the edit being minimal either way.
    proc pin_overrides {table presetname} {
        set text [edit_base]
        set old [preset_name $table]
        set plan {}
        foreach key [table_keys $table] {
            if {$key eq "name"} { continue }
            set was [raw_in $text $table $key]
            set shown [expr {$was eq "" ? [base_raw_under $table $old $key] : $was}]
            set type [::tomledit::type_of [shipped $table $key]]
            if {[same_value $type $shown [base_raw_under $table $presetname $key]]} {
                if {$was ne ""} { lappend plan unset $key {} }
            } else {
                lappend plan set $key $shown
            }
        }
        foreach {op key raw} $plan {
            if {$op eq "set"} {
                set text [::tomledit::put $text $table.$key $raw]
            } else {
                set text [::tomledit::del $text $table.$key]
            }
        }
        return [commit [write_name $text $table $presetname]]
    }

    # `name` is only worth writing when it moves the base: naming the
    # shipped preset is what an absent `name` already means.
    proc write_name {text table presetname} {
        set shippedname [::tomledit::plain [shipped $table name]]
        if {$presetname eq $shippedname} {
            return [::tomledit::del $text $table.name]
        }
        return [::tomledit::put $text $table.name \
            [::tomledit::format_value string $presetname]]
    }

    # ------------------------------------------------------ the ssh rows --
    #
    # Same discipline as the flat tables: read through the file with the
    # schema behind it, write by re-reading and editing the one row's
    # bytes. What differs is that a row can be created and destroyed, and
    # that the `default` key names a row by its `host` string rather than
    # by position, so the two move together or the check detaches.

    proc default_in {text} {
        set raw [raw_in $text ssh default]
        if {$raw eq ""} {
            set raw [shipped ssh default]
        }
        return [::tomledit::plain $raw]
    }

    # The `host` of the row a new session starts on, empty for localhost.
    proc ssh_default {} {
        variable Text
        return [default_in $Text]
    }

    proc ssh_field_default {key} {
        return [shipped ssh_host_defaults $key]
    }

    proc hosts_in {text} {
        variable SshRows
        variable SshFields
        set arrays [dict get [::tomledit::parse $text] arrays]
        if {![dict exists $arrays $SshRows]} { return {} }
        set rows {}
        foreach entry [dict get $arrays $SshRows] {
            set row [dict create]
            foreach key $SshFields {
                # A field the row does not carry is not a hole: it is what
                # `[ssh_host_defaults]` gives it, the file being a diff.
                set raw [expr {[dict exists $entry $key]
                    ? [dict get $entry $key] : [ssh_field_default $key]}]
                dict set row $key [::tomledit::plain $raw]
            }
            lappend rows $row
        }
        return $rows
    }

    # Every row in file order, each field resolved. The list's positions
    # are the indices every ssh write below takes.
    proc ssh_hosts {} {
        variable Text
        return [hosts_in $Text]
    }

    # Empty means localhost, and the minimal edit for it is to remove the
    # key rather than to write the empty string the key already defaults to.
    proc write_ssh_default {text value} {
        variable SshRows
        if {$value eq ""} {
            return [::tomledit::del $text ssh.default]
        }
        set text [::tomledit::ensure_table $text ssh $SshRows]
        return [::tomledit::put $text ssh.default \
            [::tomledit::format_value string $value]]
    }

    proc set_ssh_default {value} {
        return [commit [write_ssh_default [edit_base] $value]]
    }

    # A new row is written with its `host` alone. Every other field is what
    # `[ssh_host_defaults]` gives it, and a writer does not fill in keys at
    # their default.
    proc add_ssh_host {} {
        variable SshRows
        set text [::tomledit::add [edit_base] $SshRows \
            [list host [::tomledit::format_value string ""]]]
        return [commit $text]
    }

    # The row goes, and the check goes with it when nothing left answers to
    # its name: a `default` naming no row reads as localhost anyway, and
    # leaving it would put the check back on a row the user deleted the
    # moment another row took that name.
    proc remove_ssh_host {index} {
        variable SshRows
        set text [edit_base]
        set gone [lindex [hosts_in $text] $index]
        set text [::tomledit::del $text "$SshRows\[$index\]"]
        set host [expr {$gone eq "" ? "" : [dict get $gone host]}]
        if {$host ne "" && $host eq [default_in $text]} {
            set survives 0
            foreach row [hosts_in $text] {
                if {[dict get $row host] eq $host} { set survives 1 }
            }
            if {!$survives} { set text [write_ssh_default $text ""] }
        }
        return [commit $text]
    }

    # One field of one row. $value is a plain Tcl value, formatted after
    # the type `[ssh_host_defaults]` gives the key.
    proc set_ssh_host {index key value} {
        variable SshRows
        set text [edit_base]
        set rows [hosts_in $text]
        if {![string is integer -strict $index] || $index < 0
            || $index >= [llength $rows]} {
            error "no ssh host row at index $index"
        }
        set was [dict get [lindex $rows $index] host]
        set fallback [ssh_field_default $key]
        set type [::tomledit::type_of $fallback]
        set raw [::tomledit::format_value $type $value]
        # `host` is the row's identity and the string `default` names, so
        # it is written even when it holds the default's own empty value.
        # Every other field equal to the default is removed instead, which
        # is the minimal edit for it.
        if {$key ne "host" && [same_value $type $raw $fallback]} {
            set text [::tomledit::del $text "$SshRows\[$index\].$key"]
        } else {
            set text [::tomledit::put $text "$SshRows\[$index\].$key" $raw]
        }
        # Renaming the checked row moves the check in the same act. A
        # `default` left naming the old string would silently detach.
        if {$key eq "host" && $was ne "" && $was eq [default_in $text]} {
            set text [write_ssh_default $text $value]
        }
        return [commit $text]
    }

    proc commit {text} {
        variable Path
        variable Text
        ::tomledit::atomic_write $Path $text
        set Text $text
        return $text
    }
}
