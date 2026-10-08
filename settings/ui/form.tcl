# ::rcsettings::ui::form - the rows, and what a row does when it is moved.
#
# Every widget writes the moment it changes: there is no Apply button, because
# the running terminal watches the config file and is itself the preview. A
# slider therefore has to write repeatedly while it is dragged, which is what
# the debounce below is for: the pointer produces motion events far faster
# than a file wants rewriting, so a drag coalesces into one write every 150 ms
# and one more the instant the button comes up.
#
# What a row shows is never a widget's own memory of what it was set to. After
# any model call the row is repainted from ::rcsettings::model::effective,
# which is the file's value if the file pins the key and the named preset's
# otherwise. That is the only way the display can be right about a key the
# user just set back to its base: the minimal edit removed the key, and what
# stands in its place is the preset's value, not the value that was typed.
#
# The numeric ranges below are affordances of this window and not schema. The
# terminal clamps and interprets what it reads; a range here only says how far
# a slider travels. A value outside a range that arrives by hand-editing the
# file is displayed, not corrected.

package require Tcl 9
package require Tk

namespace eval ::rcsettings::ui::form {
    namespace export init page refresh_table refresh_all \
        maybe_fetch_system_fonts

    # table -> list of {group-title rows}, where rows is a flat list of
    # {key kind label argument} quadruples. A key written table.key belongs to
    # that table though this page lists it. Keys the docs mark as read by
    # nothing in this build are absent on purpose: general.show_menubar,
    # general.use_custom_command, general.custom_command and
    # screen.blinking_cursor would be three controls that move nothing and
    # one that lies about a cursor that does not blink. The `name` key of the
    # two axes is absent too, being the preset picker at the head of the page
    # rather than a row.
    variable Layout {
        general {
            {The knobs that are yours} {
                effects_frame_skip int   "Effects frame skip"   {1 10}
                window_scaling     scale "Window scaling"       {0.4 3.0}
                show_terminal_size bool  "Show terminal size"   {}
                bloom_quality      frac  "Bloom quality"        {}
                burn_in_quality    frac  "Burn-in quality"      {}
                led_characters     int   "LED characters"       {1 40}
                chassis_shown      bool  "Draw the chassis"     {}
                grapheme_clustering bool "Grapheme clustering"  {}
                selection_model    bool  "Between-character selection (rio)"
                    {rio konsole}
            }
        }
        screen {
            {Picture} {
                background_color color "Background"      {}
                font_color       color "Phosphor"        {}
                brightness       frac  "Brightness"      {}
                contrast         frac  "Contrast"        {}
                ambient_light    frac  "Ambient light"   {}
                saturation_color frac  "Saturation"      {}
                chroma_color     frac  "Chroma"          {}
                window_opacity   frac  "Window opacity"  {}
            }
            {Effects} {
                flickering       frac "Flickering"       {}
                horizontal_sync  frac "Horizontal sync"  {}
                static_noise     frac "Static noise"     {}
                jitter           frac "Jitter"           {}
                rgb_shift        frac "RGB shift"        {}
                glowing_line     frac "Glowing line"     {}
                burn_in          frac "Burn-in"          {}
                bloom            frac "Bloom"            {}
                screen_curvature frac "Screen curvature" {}
                rasterization    enum "Rasterization"    {}
            }
            {Type} {
                font_name    font  "Font"         {}
                general.font_size int "Font size" {6 128}
                font_source  bool  "Also offer installed system fonts"
                    {system_fonts bundled_fonts}
                font_width   scale "Font width"   {0.3 2.0}
                line_spacing frac  "Line spacing" {}
                margin       frac  "Margin"       {}
                _ note "A pixel face draws at a whole multiple of its own size; the readout shows what the chosen face draws." {}
            }
            {Moulding, shown when the chassis is not} {
                frame_size      frac  "Frame size"   {}
                screen_radius   frac  "Corner radius" {}
                frame_color     color "Frame colour" {}
                frame_shininess frac  "Shininess"    {}
            }
        }
        chassis {
            {Cabinet} {
                shell             enum "Shell"             {}
                channel_indicator enum "Channel indicator" {}
                channel_display   enum "Channel display"   {}
                bank_font_name    font "Bank font"         {}
            }
            {Moulding} {
                frame_size      frac  "Frame size"    {}
                screen_radius   frac  "Corner radius" {}
                frame_color     color "Frame colour"  {}
                frame_shininess frac  "Shininess"     {}
            }
        }
        critters {
            {When they visit} {
                enabled bool "Let them visit"      {}
                timing  enum "Timing"              {}
                minutes scale "Minutes"            {1.0 120.0}
            }
            {Who visits} {
                dolphins   bool "Dolphins"        {}
                ducks      bool "Ducks"           {}
                swan       bool "Swan"            {}
                whale      bool "Whale"           {}
                ship       bool "Ship"            {}
                monster    bool "Sea monster"     {}
                pacman     bool "Pac-Man"         {}
                locomotive bool "Locomotive"      {}
            }
        }
        serial {
            {The line} {
                enabled bool "Slow the line"                 {}
                baud    choice "Baud"
                    {300 1200 2400 4800 9600 19200 38400 57600 115200}
            }
        }
    }

    # table -> a sentence under that page's last group, for a page whose
    # rows do less than their labels promise. A note states a limit of the
    # thing being configured, never how a widget works.
    variable Notes {
        serial {A local shell only. Output reaching the glass from an SSH\
            channel, or from a tmux window under tmux -CC, has been read off\
            its transport before this terminal holds it, and is not slowed.}
    }

    # id ("table.key") -> the row's widgets and what it needs to write itself.
    variable Rows [dict create]
    # The order rows were built in, per table, so a repaint follows the page.
    variable Order [dict create]
    # table -> the preset combobox at the head of that page, for the axes.
    variable Presets [dict create]
    # The widget variables, one element per row, named by id.
    variable Value
    array set Value {}
    # Pending debounced writes, id -> after token.
    variable Pending
    array set Pending {}
    # Raised while a repaint is writing widget variables. Every -command and
    # -textvariable trace fires on a programmatic write exactly as it does on
    # a user's, so without this a repaint would write the file back out.
    variable Repainting 0

    variable StatusCmd ""
    variable WroteCmd ""

    # The installed-system-fonts catalogue: empty and unfetched until the
    # user asks for it (screen.font_source set to system_fonts) on a tab
    # whose picker would show it. Fetched is what tells "nothing installed"
    # apart from "never asked"; Fetching guards the one blocking exec against
    # a second tab visit landing while the first is still running, and is the
    # thing an error resets so the next visit tries again.
    variable SystemFonts {}
    variable SystemFontsFetched 0
    variable SystemFontsFetching 0
}

# $statuscmd is called with a message and a boolean saying whether it is an
# error; $wrotecmd is called after every write this form lands, so the window
# can re-note the file's mtime and not mistake its own edit for a hand edit.
proc ::rcsettings::ui::form::init {statuscmd wrotecmd} {
    variable StatusCmd
    variable WroteCmd
    set StatusCmd $statuscmd
    set WroteCmd $wrotecmd
}

proc ::rcsettings::ui::form::say {msg {isError 0}} {
    variable StatusCmd
    if {$StatusCmd ne ""} { {*}$StatusCmd $msg $isError }
}

# The two things a row needs a style for, neither of them decoration. The
# pinned dot is one glyph in a fixed-width column at the head of every row,
# and on a row the file does not pin it is drawn in the tab's own background
# rather than removed, so the column holds its width either way. The reset
# control is flat and unbordered because a page of twenty-six rows would
# otherwise read as a wall of buttons; it is disabled and not hidden on an
# unpinned row, a control that vanishes moving every column beside it.
proc ::rcsettings::ui::form::styles {} {
    ttk::style configure Pin.TLabel -anchor center
    ttk::style configure Unpinned.TLabel -anchor center \
        -foreground [ttk::style lookup TLabel -background]
    ttk::style configure Reset.TButton -relief flat -borderwidth 0 \
        -padding {4 0}
}

# ---------------------------------------------------------------- the page --

# Build $table's whole page under $parent and return the widget to pack.
#
# The body scrolls because the Screen page carries twenty-six rows in four
# groups and no useful window height holds them. A ttk::frame cannot scroll,
# so the classic pairing applies: a canvas scrolls, and the frame of rows
# rides inside it as a window item whose width is pinned to the canvas's.
proc ::rcsettings::ui::form::page {parent table} {
    variable Layout
    variable Order
    dict set Order $table {}

    ttk::frame $parent.page_$table
    set outer $parent.page_$table

    # The canvas asks for the space the rows want rather than taking Tk's
    # 10c by 7c default, which is narrower than a row and shorter than any
    # of the three pages. Both numbers are multiples of the line the text
    # actually measures, so the window opens the same shape at 100% and at
    # 300%, where a pixel count would open a third of the size.
    set line [font metrics TkDefaultFont -linespace]
    set canvas $outer.canvas
    canvas $canvas -highlightthickness 0 -borderwidth 0 \
        -width [expr {34 * $line}] -height [expr {26 * $line}] \
        -background [ttk::style lookup TFrame -background] \
        -yscrollcommand [list $outer.sb set]
    ttk::scrollbar $outer.sb -orient vertical -command [list $canvas yview]
    grid $canvas   -row 0 -column 0 -sticky nsew
    grid $outer.sb -row 0 -column 1 -sticky ns
    grid columnconfigure $outer 0 -weight 1
    grid rowconfigure    $outer 0 -weight 1

    set body $canvas.body
    ttk::frame $body -padding {12 10 12 12}
    set item [$canvas create window 0 0 -anchor nw -window $body]
    bind $body <Configure> [list ::rcsettings::ui::form::fit_body $canvas]
    bind $canvas <Configure> \
        [list ::rcsettings::ui::form::fit_window $canvas $item %w]
    wheel_scrolls $canvas $body

    # The two axes carry a preset picker above their groups, because every
    # row below it is measured against whatever it names.
    if {$table in {screen chassis}} { preset_picker $body $table }

    set n 0
    foreach {title rows} [dict get $Layout $table] {
        set g $body.g[incr n]
        ttk::labelframe $g -text $title -padding {10 6 10 8}
        pack $g -side top -fill x -pady {0 10}
        grid columnconfigure $g 2 -weight 1
        set r 0
        foreach {key kind label arg} $rows {
            set rowtable $table
            regexp {^(\w+)\.(.+)$} $key - rowtable key
            build_row $g $r $rowtable $key $kind $label $arg $table
            incr r
        }
    }
    variable Notes
    if {[dict exists $Notes $table]} {
        # Wrapped against the canvas's own width, which is what the rows
        # above are laid out in; a fixed pixel wraplength would break at the
        # scaling this window is built to survive.
        ttk::label $body.note -text [dict get $Notes $table] \
            -wraplength [expr {32 * [font metrics TkDefaultFont -linespace]}] \
            -justify left -padding {2 0 2 4}
        pack $body.note -side top -fill x
    }
    refresh_table $table
    return $outer
}

proc ::rcsettings::ui::form::fit_body {canvas} {
    $canvas configure -scrollregion [$canvas bbox all]
}

# The body is as wide as the canvas, so rows fill the page rather than
# hugging their own requested width, and only the height ever scrolls.
proc ::rcsettings::ui::form::fit_window {canvas item width} {
    $canvas itemconfigure $item -width $width
    fit_body $canvas
}

# The wheel over any descendant of the body scrolls the page. Tk 9 delivers
# X11 wheel presses as both <MouseWheel> with a %D and the historical
# Button-4/5, and binding the canvas alone would leave the wheel dead
# everywhere the rows actually are, so the binding goes on the body's
# bindtags and is inherited by every widget built into it.
proc ::rcsettings::ui::form::wheel_scrolls {canvas body} {
    set tag Wheel[string map {. _} $canvas]
    bind $tag <MouseWheel> \
        [list ::rcsettings::ui::form::wheel $canvas %D]
    bind $tag <Button-4> [list $canvas yview scroll -3 units]
    bind $tag <Button-5> [list $canvas yview scroll  3 units]
    bindtags $canvas [linsert [bindtags $canvas] end $tag]
    bind $body <Map> [list ::rcsettings::ui::form::inherit_wheel $body $tag]
}

proc ::rcsettings::ui::form::inherit_wheel {w tag} {
    if {$tag ni [bindtags $w]} {
        bindtags $w [linsert [bindtags $w] end $tag]
    }
    foreach child [winfo children $w] { inherit_wheel $child $tag }
}

proc ::rcsettings::ui::form::wheel {canvas delta} {
    $canvas yview scroll [expr {$delta > 0 ? -3 : 3}] units
}

# ----------------------------------------------------------------- the rows --

# One row: the pinned dot, the label, the control, a readout where the control
# has no number of its own, and the reset arrow. Five columns for every row of
# every group, so the controls line up down the page.
proc ::rcsettings::ui::form::build_row {g r table key kind label arg page} {
    variable Rows
    variable Order
    variable Value
    set id $table.$key
    if {$kind eq "note"} {
        ttk::label $g.note_$r -text $label -justify left -wraplength \
            [expr {30 * [font metrics TkDefaultFont -linespace]}]
        grid $g.note_$r -row $r -column 0 -columnspan 5 -sticky w -pady {4 0}
        return
    }

    set pin $g.pin_$key
    ttk::label $pin -text "•" -width 2 -style Unpinned.TLabel
    ttk::label $g.lbl_$key -text $label -anchor w
    set readout $g.val_$key
    ttk::label $readout -width 8 -anchor e -text ""
    set reset $g.rst_$key
    ttk::button $reset -text "↺" -width 2 -style Reset.TButton \
        -takefocus 0 -command [list ::rcsettings::ui::form::on_reset $id]

    set row [dict create table $table key $key kind $kind arg $arg \
        pin $pin readout $readout reset $reset names {}]
    set Value($id) ""

    switch -exact -- $kind {
        frac - scale {
            # A fraction is the schema's own 0.0 to 1.0; the handful of keys
            # that are not fractions carry their travel in the layout.
            set from [expr {$kind eq "frac" ? 0.0 : [lindex $arg 0]}]
            set to   [expr {$kind eq "frac" ? 1.0 : [lindex $arg 1]}]
            set w $g.ctl_$key
            ttk::scale $w -orient horizontal -from $from -to $to \
                -variable ::rcsettings::ui::form::Value($id) \
                -command [list ::rcsettings::ui::form::on_slide $id]
            # The button coming up ends the gesture, so the pending write is
            # brought forward rather than waited out: the terminal repaints
            # the moment the drag stops, not 150 ms later.
            bind $w <ButtonRelease-1> [list ::rcsettings::ui::form::on_drop $id]
            dict set row from $from
            dict set row to $to
        }
        int {
            lassign $arg from to
            set w $g.ctl_$key
            ttk::spinbox $w -from $from -to $to -increment 1 -width 6 \
                -textvariable ::rcsettings::ui::form::Value($id) \
                -command [list ::rcsettings::ui::form::on_spin $id]
            bind $w <Return>   [list ::rcsettings::ui::form::on_spin $id]
            bind $w <FocusOut> [list ::rcsettings::ui::form::on_spin $id]
            dict set row from $from
            dict set row to $to
        }
        bool {
            # arg is empty for a plain TOML boolean, whose two states this
            # checkbutton carries as 1/0 and refresh_row translates against
            # true/false. A two-element arg is a key the dump writes as a
            # string enum of two names rather than a boolean (screen's
            # font_source is one, telling apart which flag printed the font
            # catalogue), and those two names are the on/off values verbatim,
            # with no translation on refresh.
            lassign [expr {$arg eq "" ? "1 0" : $arg}] on off
            set w $g.ctl_$key
            ttk::checkbutton $w -variable ::rcsettings::ui::form::Value($id) \
                -onvalue $on -offvalue $off -text "" \
                -command [list ::rcsettings::ui::form::on_toggle $id]
        }
        enum - font - choice {
            # `enum` is a key the schema states as an enum, and its names
            # come from the dump's own list. `choice` is a key of ordinary
            # type whose usable values are a convention rather than a
            # schema fact, and the row states them itself: serial's baud is
            # the rates a wire ran at, over an integer the terminal would
            # take any value of.
            if {$kind eq "font"} {
                set names [font_keys]
            } elseif {$kind eq "choice"} {
                set names $arg
            } else {
                set names [::rcsettings::model::enum $key]
            }
            set shown [expr {$kind eq "font" ? [font_labels] : $names}]
            set w $g.ctl_$key
            ttk::combobox $w -state readonly -values $shown -exportselection 0
            bind $w <<ComboboxSelected>> \
                [list ::rcsettings::ui::form::on_pick $id]
            dict set row names $names
        }
        color {
            # A swatch is the colour, so it is a canvas filled with it rather
            # than a button labelled with a hex string. ttk has no background
            # option to fill a label with an arbitrary colour per widget,
            # which is why this one control is not a ttk widget.
            set w $g.ctl_$key
            # The edge is a fixed neutral grey rather than a colour of the
            # window's: a swatch has to have an outline to be a swatch at all,
            # and a value of #000000 against a light background otherwise has
            # no edge to show where it ends.
            canvas $w -width 56 -height 18 -highlightthickness 1 \
                -highlightbackground #808080 \
                -borderwidth 0 -cursor hand2
            bind $w <Button-1> [list ::rcsettings::ui::form::on_colour $id]
        }
        default { error "unknown row kind \"$kind\" for $id" }
    }
    dict set row control $w

    grid $pin          -row $r -column 0 -sticky w
    grid $g.lbl_$key   -row $r -column 1 -sticky w -padx {0 10}
    grid $w            -row $r -column 2 -sticky [expr {
        $kind in {frac scale enum font} ? "ew" : "w"}] -pady 1
    grid $readout      -row $r -column 3 -sticky e -padx {8 4}
    grid $reset        -row $r -column 4 -sticky e

    dict set Rows $id $row
    dict lappend Order $page $id
}

# The font catalogue, as the dump lists it: the persisted key and the label
# the terminal shows for it, which are not the same string for the bundled
# faces (TERMINESS_SCALED prints as "Terminess"). The installed system faces,
# once fetched, are appended after the bundled ones rather than merged by
# name: a system install sharing a bundled face's own name is still a
# different key, and only position (not the printed words) tells the two
# apart, the same reasoning on_pick already relies on.
# Column $i (0 the catalogue key, 1 the display name) of the bundled
# fonts followed by the system ones, which is the one list font_keys and
# font_labels each pick a column out of.
proc ::rcsettings::ui::form::font_column {i} {
    variable SystemFonts
    set out {}
    foreach f [::rcsettings::model::fonts] {
        lappend out [lindex $f $i]
    }
    foreach f $SystemFonts { lappend out [lindex $f $i] }
    return $out
}

proc ::rcsettings::ui::form::font_keys {} { return [font_column 0] }
proc ::rcsettings::ui::form::font_labels {} { return [font_column 1] }

# ----------------------------------------------------- system font fetch --

# Called on a notebook tab change (with the table the newly-selected tab
# shows) and from the checkbox's own toggle. A no-op unless all three hold:
# nobody has fetched yet, the box is actually checked, and the page looking
# at a font picker is the one now showing. The exec is blocking, like every
# other write in this file, so the status line is what tells the user
# something is happening before it lands.
proc ::rcsettings::ui::form::maybe_fetch_system_fonts {table} {
    variable SystemFonts
    variable SystemFontsFetched
    variable SystemFontsFetching
    if {$SystemFontsFetched || $SystemFontsFetching} { return }
    if {[::rcsettings::model::effective screen font_source] ne "system_fonts"} {
        return
    }
    if {$table ni {screen chassis}} { return }
    set SystemFontsFetching 1
    say "reading installed fonts…"
    if {[catch {::rcsettings::model::system_fonts} fonts]} {
        say "cannot read installed fonts: $fonts" 1
        set SystemFontsFetching 0
        return
    }
    set SystemFonts $fonts
    set SystemFontsFetched 1
    set SystemFontsFetching 0
    update_font_combos
}

# The two pickers whose list is the merged catalogue: screen.font_name and
# chassis.bank_font_name. Their -values move in place, and each is
# re-resolved against the wider list, which is how a persisted system face
# shown as "missing" under the bundled-only list resolves the moment the
# merge gives it back its name.
proc ::rcsettings::ui::form::update_font_combos {} {
    variable Rows
    set names [font_keys]
    set shown [font_labels]
    foreach id {screen.font_name chassis.bank_font_name} {
        if {![dict exists $Rows $id]} { continue }
        set row [dict get $Rows $id]
        dict set row names $names
        dict set Rows $id $row
        [dict get $row control] configure -values $shown
        refresh_row $id
    }
}

# ------------------------------------------------------------ what a row does --

# Writing through the model, with the one failure this window can meet: the
# config directory is not writable, or the rename lost a race. The row is
# repainted either way, so what the user sees after a failed write is what the
# file actually holds and not the value they aimed at.
proc ::rcsettings::ui::form::write {id script} {
    if {[catch {uplevel 1 $script} err]} {
        say "cannot write [::rcsettings::model::path]: $err" 1
        refresh_row $id
        return 0
    }
    variable WroteCmd
    if {$WroteCmd ne ""} { {*}$WroteCmd }
    refresh_row $id
    return 1
}

proc ::rcsettings::ui::form::set_value {id value} {
    variable Rows
    set row [dict get $Rows $id]
    write $id [list ::rcsettings::model::set_value \
        [dict get $row table] [dict get $row key] $value]
}

proc ::rcsettings::ui::form::on_reset {id} {
    variable Rows
    set row [dict get $Rows $id]
    set table [dict get $row table]
    set key [dict get $row key]
    if {![::rcsettings::model::pinned $table $key]} { return }
    if {[write $id [list ::rcsettings::model::reset $table $key]]} {
        say "$table.$key back to [::rcsettings::model::preset_name $table]"
    }
}

# A slider being dragged: the readout follows the pointer at once, because a
# number that lagged the handle would read as the window being slow, and the
# file is written on a timer behind it.
proc ::rcsettings::ui::form::on_slide {id value} {
    variable Repainting
    variable Pending
    if {$Repainting} { return }
    show_number $id $value
    if {[info exists Pending($id)]} { after cancel $Pending($id) }
    set Pending($id) [after 150 [list ::rcsettings::ui::form::flush_slide $id]]
}

proc ::rcsettings::ui::form::on_drop {id} {
    variable Pending
    if {[info exists Pending($id)]} {
        after cancel $Pending($id)
        unset Pending($id)
    }
    flush_slide $id
}

# Two decimals, because a slider's pixel is worth about a hundredth of its
# travel and the file is a document a person reads: 0.35 is a value someone
# chose, 0.3499999940395355 is a widget's internal state leaking into it.
proc ::rcsettings::ui::form::flush_slide {id} {
    variable Pending
    variable Value
    array unset Pending $id
    set_value $id [format %.2f $Value($id)]
}

proc ::rcsettings::ui::form::on_spin {id} {
    variable Repainting
    variable Rows
    variable Value
    if {$Repainting} { return }
    set row [dict get $Rows $id]
    set v [string trim $Value($id)]
    # A spinbox is typable, so it can hold anything. Nonsense is not written
    # and not corrected in place either; the row is repainted, which puts the
    # value the file holds back in the box.
    if {![string is integer -strict $v]} { refresh_row $id; return }
    if {$v < [dict get $row from]} { set v [dict get $row from] }
    if {$v > [dict get $row to]}   { set v [dict get $row to] }
    set_value $id $v
}

proc ::rcsettings::ui::form::on_toggle {id} {
    variable Repainting
    variable Value
    if {$Repainting} { return }
    set_value $id $Value($id)
    # The checkbox only ever sits on the Screen page, so opting in while
    # looking at it is opting in while the tab the fetch is gated on is
    # already the one showing: no need to ask the notebook which tab this is.
    if {$id eq "screen.font_source"} { maybe_fetch_system_fonts screen }
}

proc ::rcsettings::ui::form::on_pick {id} {
    variable Repainting
    variable Rows
    if {$Repainting} { return }
    set row [dict get $Rows $id]
    set i [[dict get $row control] current]
    if {$i < 0} { return }
    # By index, not by the text shown. The font catalogue holds two entries
    # that print the same words (the bundled JetBrains Mono and a system
    # install of it), and only their position tells them apart.
    set_value $id [lindex [dict get $row names] $i]
    if {$id eq "screen.font_name"} { refresh_row general.font_size }
}

proc ::rcsettings::ui::form::on_colour {id} {
    variable Rows
    set row [dict get $Rows $id]
    set current [::rcsettings::model::effective \
        [dict get $row table] [dict get $row key]]
    set picked [tk_chooseColor -parent [dict get $row control] \
        -title [dict get $row key] \
        -initialcolor [expr {[valid_colour $current] ? $current : "#000000"}]]
    if {$picked eq ""} { return }
    set_value $id [hex $picked]
}

# tk_chooseColor answers in whatever width the display carries, which is
# 16 bits per channel on this one (#ffff81000000). The config file's colours
# are three bytes, so the answer is narrowed rather than written through.
proc ::rcsettings::ui::form::hex {colour} {
    lassign [winfo rgb . $colour] r g b
    return [format "#%02x%02x%02x" [expr {$r >> 8}] [expr {$g >> 8}] [expr {$b >> 8}]]
}

proc ::rcsettings::ui::form::valid_colour {text} {
    return [expr {![catch {winfo rgb . $text}]}]
}

# ------------------------------------------------------------------ repaint --

proc ::rcsettings::ui::form::refresh_all {} {
    variable Order
    foreach table [dict keys $Order] { refresh_table $table }
}

proc ::rcsettings::ui::form::refresh_table {table} {
    variable Order
    variable Presets
    if {[dict exists $Presets $table]} { refresh_preset $table }
    foreach id [dict get $Order $table] { refresh_row $id }
}

proc ::rcsettings::ui::form::refresh_row {id} {
    variable Rows
    variable Repainting
    variable Value
    set row [dict get $Rows $id]
    set table [dict get $row table]
    set key [dict get $row key]
    set w [dict get $row control]

    if {[catch {::rcsettings::model::effective $table $key} value]} {
        say "cannot read $table.$key: $value" 1
        return
    }
    set pinned [::rcsettings::model::pinned $table $key]

    incr Repainting
    try {
        switch -exact -- [dict get $row kind] {
            frac - scale {
                set Value($id) [expr {[string is double -strict $value] ? $value : 0}]
                show_number $id $Value($id)
            }
            int {
                set Value($id) $value
                [dict get $row readout] configure -text [expr {
                    $id eq "general.font_size" ? [size_readout $value] : ""}]
            }
            bool {
                # A plain boolean (empty arg) is what the file spells
                # true/false, translated to the checkbutton's 1/0. A
                # two-name arg is already one of the checkbutton's own
                # on/off strings, so it goes into the variable plain.
                if {[dict get $row arg] eq ""} {
                    set Value($id) [expr {$value eq "true" ? 1 : 0}]
                } else {
                    set Value($id) $value
                }
                [dict get $row readout] configure -text ""
            }
            enum - font - choice {
                set i [lsearch -exact [dict get $row names] $value]
                if {$i >= 0} {
                    $w current $i
                } else {
                    # A name the catalogue does not carry is what the file
                    # says, and the terminal falls back to its default face
                    # without rewriting the file. Showing it is more honest
                    # than showing the face that will actually be drawn.
                    $w set $value
                }
                [dict get $row readout] configure -text ""
            }
            color {
                if {[valid_colour $value]} {
                    $w configure -background $value
                } else {
                    $w configure \
                        -background [ttk::style lookup TFrame -background]
                }
                [dict get $row readout] configure -text $value
            }
        }
    } finally {
        incr Repainting -1
    }

    [dict get $row pin] configure \
        -style [expr {$pinned ? "Pin.TLabel" : "Unpinned.TLabel"}]
    [dict get $row reset] state [expr {$pinned ? "!disabled" : "disabled"}]
}

# What the chosen face draws at display scale 1: a pixel face is magnified by
# the whole number nearest the setting.
proc ::rcsettings::ui::form::size_readout {n} {
    if {![string is integer -strict $n]} { return "" }
    lassign [::rcsettings::model::face_size \
        [::rcsettings::model::effective screen font_name]] size low
    if {$low ne "true"} { return "$n px" }
    set m [expr {max(1, round(double($n) / $size))}]
    return "[expr {$size * $m}] px ($size × $m)"
}

proc ::rcsettings::ui::form::show_number {id value} {
    variable Rows
    [dict get $Rows $id readout] configure -text [format %.2f $value]
}

# ------------------------------------------------------------- the presets --

proc ::rcsettings::ui::form::preset_picker {body table} {
    variable Presets
    set f $body.preset
    ttk::frame $f
    pack $f -side top -fill x -pady {0 10}
    ttk::label $f.lbl -text "Preset" -anchor w
    ttk::combobox $f.cb -state readonly -exportselection 0
    pack $f.lbl -side left -padx {0 10}
    pack $f.cb -side left -fill x -expand 1
    bind $f.cb <<ComboboxSelected>> \
        [list ::rcsettings::ui::form::on_preset $table]
    dict set Presets $table $f.cb
}

proc ::rcsettings::ui::form::refresh_preset {table} {
    variable Presets
    variable Repainting
    set cb [dict get $Presets $table]
    set names [::rcsettings::model::preset_names $table]
    set current [::rcsettings::model::preset_name $table]
    # A look the user saved under a name of their own is not a built-in and
    # is still what the table resolves against, so it joins the list rather
    # than leaving the picker showing someone else's name.
    if {$current ni $names} { lappend names $current }
    incr Repainting
    try {
        $cb configure -values $names
        $cb set $current
    } finally {
        incr Repainting -1
    }
}

# Choosing a preset is two different edits and the user is the only one who
# knows which: switching to what the preset looks like, or renaming the look
# they have built so it keeps its values under the new base. With nothing
# pinned the two are the same edit, so the question is not asked.
proc ::rcsettings::ui::form::on_preset {table} {
    variable Presets
    variable Repainting
    if {$Repainting} { return }
    set cb [dict get $Presets $table]
    set chosen [$cb get]
    if {$chosen eq [::rcsettings::model::preset_name $table]} { return }
    if {![has_overrides $table]} {
        apply_preset $table switch $chosen
        return
    }
    ::rcsettings::ui::preset_dialog::open $cb $table $chosen \
        [list ::rcsettings::ui::form::apply_preset $table]
}

proc ::rcsettings::ui::form::has_overrides {table} {
    foreach key [::rcsettings::model::table_keys $table] {
        if {$key eq "name"} { continue }
        if {[::rcsettings::model::pinned $table $key]} { return 1 }
    }
    return 0
}

# $answer is switch, keep or cancel. Cancel still repaints, because the
# picker is showing the name that was chosen and the table did not move.
proc ::rcsettings::ui::form::apply_preset {table answer chosen} {
    variable WroteCmd
    switch -exact -- $answer {
        switch {
            if {[catch {::rcsettings::model::switch_preset $table $chosen} err]} {
                say "cannot write [::rcsettings::model::path]: $err" 1
            } else {
                if {$WroteCmd ne ""} { {*}$WroteCmd }
                say "$table is now $chosen"
            }
        }
        keep {
            if {[catch {::rcsettings::model::pin_overrides $table $chosen} err]} {
                say "cannot write [::rcsettings::model::path]: $err" 1
            } else {
                if {$WroteCmd ne ""} { {*}$WroteCmd }
                say "$table kept its look under the name $chosen"
            }
        }
    }
    # The whole form, not the one table: a preset switch rewrites what every
    # key in it falls back to, and the cheapest way to be right about that is
    # to ask the model again for every row there is.
    refresh_all
}
