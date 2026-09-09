# What the finished image can be asked about itself: robco-term --settings
# --selftest, or the standalone image with --selftest, prints a verdict and
# leaves with 0 or 1.
#
# The failures this catches are the packaging ones, which are invisible to a
# suite run against the checkout: a script library that did not get staged, a
# Tk that is linked but not registered, a lib/ file that is in the repository
# and not in the archive. They are also exactly the failures that, on a
# Windows GUI launch, show the user nothing at all - so a build can run this
# and find out before a user does.
#
# It runs before the window is built and touches no display, so it answers on
# a headless CI runner.

namespace eval ::selftest {
    variable root [file dirname [file normalize [info script]]]
    variable problems {}
    variable passed 0
}
source [file join $::selftest::root lib diag.tcl]

# Everything said here is said twice: once to whoever ran it, and once into
# the log, because the run that matters most is the one whose output nobody
# was watching.
proc ::selftest::say {text} {
    puts stdout $text
    # The message stands as its own detail: there is no stack behind a
    # verdict, and the interpreter's errorInfo here belongs to whatever the
    # suites last provoked on purpose.
    ::rcsettings::diag::record selftest $text $text
}

proc ::selftest::check {what script} {
    variable problems
    variable passed
    if {[catch {uplevel 1 $script} result]} {
        lappend problems "$what: $result"
        return 0
    }
    if {!$result} {
        lappend problems $what
        return 0
    }
    incr passed
    return 1
}

# 1. The interpreter's own two script libraries. Without the first there is
#    no Tcl to speak of; without the second, Tk loads and then fails on the
#    first widget.
::selftest::check "tcl_library/init.tcl is in the image" {
    file readable [file join $::selftest::root tcl_library init.tcl]
}
::selftest::check "tk_library/tk.tcl is in the image" {
    file readable [file join $::selftest::root tk_library tk.tcl]
}

# 2. Tk itself, which in this build is linked in rather than loaded from a
#    shared object: the question is whether the interpreter knows it is
#    there, which both of these answer without drawing anything. A statically
#    registered Tk has provided its version and left `package versions`
#    empty, having no ifneeded script to be loaded by; one waiting on disk
#    has the versions and not the provide.
::selftest::check "Tk is registered with the interpreter" {
    expr {[package provide Tk] ne "" || [package versions Tk] ne ""}
}

# 3. The app's own Tcl-only libraries, sourced in the order the launcher
#    sources them. A file missing from the archive fails here rather than
#    halfway through drawing a window.
foreach ::selftest::lib {diag.tcl tomledit-1.1a1.tm model.tcl} {
    ::selftest::check "lib/$::selftest::lib sources" [format {
        source [file join $::selftest::root lib %s]
        expr {1}
    } $::selftest::lib]
}

# 4. The suites, run from inside the image against the image's own copies.
#    tcltest is a module, staged only when the build was told where to find
#    one; without it the image is still good and simply cannot test itself,
#    which is said plainly rather than counted as a failure.
if {[catch {package require tcltest 2.5} ::selftest::tcltesterr]} {
    ::selftest::say "settings selftest: no tcltest in this image,\
        the suites were not run"
} else {
    # The files are sourced here rather than handed to runAllTests, which
    # would either start a child process per file - there is no interpreter
    # on disk to start - or, having Tk loaded, exit the process out from
    # under this script when it reported. testSingleFile false is what tells
    # each file's own cleanupTests to defer its report to us.
    ::tcltest::configure -testdir [file join $::selftest::root tests]
    ::tcltest::configure -tmpdir [file tempdir robco-settings-selftest]
    set ::tcltest::testSingleFile false
    foreach ::selftest::suite {model.test} {
        set ::selftest::path \
            [file join $::selftest::root tests $::selftest::suite]
        if {[catch {source $::selftest::path} ::selftest::err]} {
            lappend ::selftest::problems \
                "tests/$::selftest::suite: $::selftest::err"
        }
    }
    incr ::selftest::passed $::tcltest::numTests(Passed)
    if {$::tcltest::numTests(Failed) > 0} {
        lappend ::selftest::problems \
            "$::tcltest::numTests(Failed) test(s) failed in\
            [join $::tcltest::failFiles {, }]"
    }
}

if {[llength $::selftest::problems] == 0} {
    ::selftest::say "settings selftest ok: $::selftest::passed passed"
    exit 0
}
foreach ::selftest::problem $::selftest::problems {
    ::selftest::say "settings selftest FAILED: $::selftest::problem"
}
::selftest::say "settings selftest failed:\
    [llength $::selftest::problems] problem(s), $::selftest::passed passed"
exit 1
