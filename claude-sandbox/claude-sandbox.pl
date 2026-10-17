#!/usr/bin/env perl
#
# Shamelessly derived from original work at:
#
# https://raw.githubusercontent.com/paulsmith/claude-sandbox/refs/heads/main/claude-sandbox
#
# Copyright (c) 2025 Paul Smith <paulsmith@pobox.com>
# SPDX-License-Identifier: MIT
#
# claude-sandbox -- run Claude Code inside a tighter macOS sandbox
#
#   claude-sandbox                # generate profile + run claude (default)
#   claude-sandbox run [...args]  # same, pass args through
#   claude-sandbox shell          # generate profile + run $SHELL
#   claude-sandbox generate       # build the profile only
#   claude-sandbox profile        # print last-generated profile path
#   claude-sandbox check path...  # report which rule decides each access
#   claude-sandbox help           # this text
#
#   claude-sandbox -f file ...    # use "file" instead of ~/.claude-sandbox.conf
#   claude-sandbox -x ...         # extra-verbose: one rule line per path
#
# User rules live in ~/.claude-sandbox.conf:
#
#   # comment (copied into the profile as ";;")
#   [+|-][rwxima] path [path ...]
#   @rewrite from to
#
#   r=file-read*  w=file-write*  x=process-exec  i=file-ioctl
#   m=file-map-executable  a=all of them
#   "+" always includes "r"; no option at all means "+r".
#   "-" with no letters means "-a"; otherwise only the given letters.
#
#   Paths: "/..." is absolute, "~" is $HOME, anything else is relative
#   to $HOME.  Directories become "subpath", files "literal"; a trailing
#   "/" forces subpath, a leading "=" forces literal.  Quote paths with
#   spaces ("a b").  r"..." is a regex (no relative paths).
#   "${NAME}" expands anywhere in a path or regex (escaped in a regex)
#   from the environment, plus "${CWD}" (the workspace); "${HOME}" and
#   "${TMPDIR}" are normalized.  An unset or empty variable is an error,
#   unless the path starts with "?" (e.g. "+rwx ?${FOO}"), which drops
#   it.  "$" must be followed by "{" (use "\$" in quotes for a real "$"),
#   except in a regex, where it is the usual end anchor.
#   Symlinks are followed and every path in the chain is emitted.
#   "@rewrite A B" adds B for every path under A, and vice versa.
#
#   Rule order (last match wins): system rules (including "-a ~"),
#   "+rwxm ${CWD}", the conf file in order, then forced denies.
#
# Note: sandbox-exec(1) is deprecated but works on macOS 14 + 15 (June 2025).
#-------------------------------------------------------------------------------

use strict;
use warnings;
use Cwd qw(abs_path getcwd);
use File::Basename;
use POSIX qw(strftime);

# 1. Early Check for sandbox-exec availability
system("command -v sandbox-exec >/dev/null 2>&1");
if ($? != 0) {
    print STDERR "Error: sandbox-exec not found. This tool requires macOS with sandbox-exec.\n";
    exit 1;
}

# 2. Setup Variables and Path Hashing (replicates 'pwd -P | cksum')
my $project_dir  = abs_path(getcwd());
my $home_dir     = $ENV{HOME};
my $tmp_dir      = $ENV{TMPDIR} || '/tmp';
my $conf_file    = "$home_dir/.claude-sandbox.conf";

my $one_per_line = 0;  # -x: one "(allow ...)" line per path

# Leading options, before the command
while (@ARGV && ($ARGV[0] eq '-f' || $ARGV[0] eq '-x')) {
    my $opt = shift @ARGV;
    if ($opt eq '-x') {
        $one_per_line = 1;
        next;
    }
    unless (@ARGV) {
        print STDERR "Error: -f needs a config file argument\n";
        exit 1;
    }
    $conf_file = shift @ARGV;
    $conf_file = getcwd() . "/$conf_file" unless $conf_file =~ m|^/|;
}

my $base_name    = basename($project_dir);
my $cksum        = POSIX_cksum($project_dir);
my $hex_hash     = sprintf("%x", $cksum);

my $profile_name = "claude-sandbox-$base_name-$hex_hash";
my $profile_file = "$tmp_dir/$profile_name.sb";

# Built-in "${NAME}" variables, which take precedence over the environment
my %VARS = (
    CWD    => norm_path($project_dir),
    HOME   => norm_path($home_dir),
    TMPDIR => norm_path($tmp_dir),
);

# Permission letters, in emit order, and their sandbox operations.
# "M" (file-read-metadata) is internal: implied by "r", and used for
# the stat-only allows emitted for parent directories.
my $CATS = 'rwxim';
my %CAT_OP = (
    r => 'file-read*',
    w => 'file-write*',
    x => 'process-exec',
    i => 'file-ioctl',
    m => 'file-map-executable',
    M => 'file-read-metadata',
);
my %CAT_NAME = (
    M => 'stat', r => 'read', w => 'write', x => 'exec',
    i => 'ioctl', m => 'map-exec',
);

my @rewrites;           # [from, to] pairs, from @rewrite directives

# 3. Command dispatcher
my $cmd = shift @ARGV || 'run';

if ($cmd eq 'shell') {
    run_claude($ENV{SHELL});
} elsif ($cmd eq 'generate') {
    generate_profile();
    print "$profile_file\n";
} elsif ($cmd eq 'profile') {
    print "$profile_file\n";
} elsif ($cmd eq 'check') {
    check_paths(@ARGV);
} elsif ($cmd eq 'help' || $cmd eq '-h' || $cmd eq '--help') {
    usage();
} elsif ($cmd eq 'claude') {
    run_claude("claude", @ARGV);
} elsif ($cmd eq 'run') {
    run_claude("claude", @ARGV);
} else {
    run_claude("claude", $cmd, @ARGV);
}

# --- Functions ---

sub usage {
    # Print the top comment block, from the title line to the dashes
    open(my $fh, '<', $0) or die "Cannot open script: $!";
    my $on = 0;
    while (my $line = <$fh>) {
        $on = 1 if $line =~ /^# claude-sandbox /;
        last if $line =~ /^#-{10}/;
        next unless $on;
        $line =~ s/^# ?//;
        print $line;
    }
    close($fh);
}

sub POSIX_cksum {
    # Replicates POSIX cksum algorithm on a string
    my ($str) = @_;
    my $len = length($str);
    my $crc = 0;
    for (my $i = 0; $i < $len; $i++) {
        my $c = ord(substr($str, $i, 1));
        $crc = (($crc << 8) & 0xFFFFFFFF) ^ _cksum_table([($crc >> 24) ^ $c]->[0]);
    }
    while ($len > 0) {
        $crc = (($crc << 8) & 0xFFFFFFFF) ^ _cksum_table([($crc >> 24) ^ ($len & 0xFF)]->[0]);
        $len >>= 8;
    }
    return ~$crc & 0xFFFFFFFF;
}

sub _cksum_table {
    my ($idx) = @_;
    my $poly = 0x04C11DB7;
    my $crc = $idx << 24;
    for (my $i = 0; $i < 8; $i++) {
        if ($crc & 0x80000000) {
            $crc = (($crc << 1) ^ $poly) & 0xFFFFFFFF;
        } else {
            $crc = ($crc << 1) & 0xFFFFFFFF;
        }
    }
    return $crc;
}

#-------------------------------------------------------------------------------
# Path helpers

# Textual cleanup: collapse "//", drop "/./", fold "..", strip trailing "/"
sub norm_path {
    my ($p) = @_;
    my @out;
    foreach my $c (split m|/|, $p) {
        next if $c eq '' || $c eq '.';
        if ($c eq '..') {
            pop @out;
        } else {
            push @out, $c;
        }
    }
    return '/' . join('/', @out);
}

# Follow symlinks one at a time, leftmost first, returning every path
# in the chain: /foo -> /tmp/foo -> /private/tmp/foo
sub expand_links {
    my ($path) = @_;
    my @out = ($path);
    my $cur = $path;

    for (1 .. 32) {
        my @c = split m|/|, $cur;
        my $prefix = '';
        my $next;

        for my $i (1 .. $#c) {
            $prefix .= "/$c[$i]";
            next unless -l $prefix;
            my $target = readlink($prefix);
            last unless defined $target;
            $target = dirname($prefix) . "/$target" unless $target =~ m|^/|;
            my $rest = join('/', @c[$i + 1 .. $#c]);
            $next = norm_path($target . (length($rest) ? "/$rest" : ''));
            last;
        }

        last unless defined $next;
        last if grep { $_ eq $next } @out;      # symlink loop
        push @out, $next;
        $cur = $next;
    }
    return @out;
}

# Apply @rewrite pairs, in both directions
sub apply_rewrites {
    my (@paths) = @_;
    my @out;
    foreach my $p (@paths) {
        push @out, $p;
        foreach my $rw (@rewrites) {
            foreach my $dir ([$rw->[0], $rw->[1]], [$rw->[1], $rw->[0]]) {
                my ($from, $to) = @$dir;
                if ($p eq $from || index($p, "$from/") == 0) {
                    push @out, $to . substr($p, length($from));
                }
            }
        }
    }
    my %seen;
    return grep { !$seen{$_}++ } @out;
}

# Escape a string for use inside a POSIX extended regex
sub ere_escape {
    my ($s) = @_;
    $s =~ s/([.\[\]()*+?{}|^\$\\])/\\$1/g;
    return $s;
}

# Apply @rewrite pairs to a regex that starts with "^<path>"
sub apply_regex_rewrites {
    my ($rx) = @_;
    my @out = ($rx);
    foreach my $rw (@rewrites) {
        foreach my $dir ([$rw->[0], $rw->[1]], [$rw->[1], $rw->[0]]) {
            my ($from, $to) = @$dir;
            my $efrom = '^' . ere_escape($from);
            next unless index($rx, $efrom) == 0;
            my $next = substr($rx, length($efrom), 1);
            next unless $next eq '' || $next eq '/' || $next eq '$'
                || $next eq '(';
            push @out, '^' . ere_escape($to) . substr($rx, length($efrom));
        }
    }
    my %seen;
    return grep { !$seen{$_}++ } @out;
}

# Value of "${NAME}": our own variables first, then the environment
sub var_value {
    my ($name) = @_;
    return $VARS{$name} if exists $VARS{$name};
    die "environment variable \"$name\" is not set\n"
        unless defined $ENV{$name};
    die "environment variable \"$name\" is empty\n" if $ENV{$name} eq '';
    return $ENV{$name};
}

# Expand "${NAME}" anywhere in a regex, escaping the value
sub expand_regex_vars {
    my ($rx) = @_;
    $rx =~ s/\$\{(\w+)\}/ere_escape(var_value($1))/ge;
    return $rx;
}

# Turn a conf path token into an absolute path; dies on error.
# Returns (path, forced-subpath)
sub resolve_conf_path {
    my ($v) = @_;

    $v =~ s/\$\{(\w+)\}/var_value($1)/ge;
    $v =~ s/\0/\$/g;            # "\$" from a quoted path

    if ($v =~ m{^~(/.*)?$}) {
        $v = $home_dir . ($1 // '');
    } elsif ($v =~ m{^~}) {
        die "\"~user\" paths are not supported\n";
    }

    $v = "$home_dir/$v" unless $v =~ m|^/|;
    my $sub = ($v =~ m|./$|) ? 1 : 0;
    return (norm_path($v), $sub);
}

#-------------------------------------------------------------------------------
# Conf parsing

# Split the rest of a line into path tokens
sub tokenize {
    my ($s) = @_;
    my @t;

    while (1) {
        $s =~ s/^\s+//;
        last if $s eq '' || $s =~ /^#/;

        my $before = $s;
        my $opt = ($s =~ s/^\?//) ? 1 : 0;
        my $tok;

        if ($s =~ s/^r"((?:[^"\\]|\\.)*)"//) {
            $tok = { type => 'regex', value => $1 };
        } elsif ($s =~ s/^(=?)"((?:[^"\\]|\\.)*)"//) {
            my ($lit, $v) = ($1, $2);
            # "\$" is a real "$"; hide it from variable expansion
            $v =~ s/\\(.)/$1 eq '$' ? "\0" : $1/ge;
            $tok = { type => 'path', value => $v, literal => $lit ne '' };
        } elsif ($s =~ s/^(=?)([^\s"]+)//) {
            $tok = { type => 'path', value => $2, literal => $1 ne '' };
        } else {
            die "unterminated quote\n";
        }

        die "missing whitespace after path\n" if $s ne '' && $s !~ /^\s/;
        check_dollars($tok->{value}) if $tok->{type} eq 'path';

        $tok->{optional} = $opt;
        $tok->{raw} = substr($before, 0, length($before) - length($s));
        push @t, $tok;
    }
    return @t;
}

# In a path, "$" must start a well-formed "${NAME}"
sub check_dollars {
    my ($v) = @_;
    die "unterminated \"\${\"\n" if $v =~ /\$\{(?!\w+\})/;
    die "use \"\${$1}\" instead of \"\$$1\"\n" if $v =~ /\$(\w+)/;
    die "\"\$\" must be followed by \"{\"\n" if $v =~ /\$(?!\{)/;
}

# For an optional ("?") token: why it should be skipped, if it should
# (some "${NAME}" it uses is unset or empty).  Returns a list of reasons.
sub skip_reasons {
    my ($tok) = @_;
    return () unless $tok->{optional};

    my @why;
    my $v = $tok->{value};
    while ($v =~ /\$\{(\w+)\}/g) {
        my $name = $1;
        next if exists $VARS{$name};
        if (!defined $ENV{$name}) {
            push @why, "$name not set";
        } elsif ($ENV{$name} eq '') {
            push @why, "$name is empty";
        }
    }
    my %seen;
    return grep { !$seen{$_}++ } @why;
}

# Drop the optional tokens that should be skipped; returns the kept
# tokens and a list of "token (reason)" strings for the skipped ones
sub drop_skipped {
    my (@t) = @_;
    my (@keep, @skipped);
    foreach my $tok (@t) {
        my @why = skip_reasons($tok);
        if (@why) {
            push @skipped, "$tok->{raw} (" . join(', ', @why) . ")";
        } else {
            push @keep, $tok;
        }
    }
    return (\@keep, \@skipped);
}

# Parse conf text into entries: comments and unresolved rules.
# @rewrite directives are collected into @rewrites as a side effect.
sub parse_conf {
    my ($text, $label, $errors) = @_;
    my @entries;
    my $lineno = 0;

    foreach my $line (split /\n/, $text) {
        $lineno++;
        my $src = "$label:$lineno";
        $line =~ s/^\s+//;
        $line =~ s/\s+$//;

        if ($line eq '') {
            push @entries, { kind => 'blank' };
            next;
        }

        if ($line =~ /^#\s?(.*)$/) {
            push @entries, { kind => 'comment', text => $1 };
            next;
        }

        my $ok = eval {
            if ($line =~ /^@(\S*)\s*(.*)$/) {
                my ($dir, $rest) = ($1, $2);
                die "unknown directive \"\@$dir\"\n" unless $dir eq 'rewrite';
                my @t = tokenize($rest);
                die "\@rewrite needs exactly two paths\n" unless @t == 2;
                die "\@rewrite takes plain paths\n"
                    if grep { $_->{type} ne 'path' || $_->{literal} } @t;

                my ($keep, $skipped) = drop_skipped(@t);
                if (@$skipped) {
                    push @entries, { kind => 'comment',
                        text => "$src: $line\n;;   skipped: "
                            . join("\n;;   skipped: ", @$skipped) };
                    return 1;
                }

                my ($from) = resolve_conf_path($t[0]{value});
                my ($to) = resolve_conf_path($t[1]{value});
                push @rewrites, [$from, $to];
                push @entries, { kind => 'comment', text => "$src: $line" };
                return 1;
            }

            my ($sign, $letters, $rest) = ('+', '', $line);
            if ($line =~ /^([+-])(\S*)\s*(.*)$/) {
                ($sign, $letters, $rest) = ($1, $2, $3);
                foreach my $c (split //, $letters) {
                    die "unknown option letter \"$c\"\n"
                        unless index('rwxima', $c) >= 0;
                }
            }

            my %cats;
            if ($letters =~ /a/ || ($sign eq '-' && $letters eq '')) {
                $cats{$_} = 1 foreach split //, $CATS;
            } else {
                $cats{$_} = 1 foreach split //, $letters;
                $cats{r} = 1 if $sign eq '+';
            }
            $cats{M} = 1 if $cats{r};

            my @t = tokenize($rest);
            die "no paths given\n" unless @t;

            my ($keep, $skipped) = drop_skipped(@t);
            @t = @$keep;

            # Every path was optional and skipped: leave a trace
            unless (@t) {
                push @entries, { kind => 'comment',
                    text => "$src: $line\n;;   skipped: "
                        . join("\n;;   skipped: ", @$skipped) };
                return 1;
            }

            foreach my $tok (@t) {
                if ($tok->{type} eq 'regex') {
                    $tok->{value} = expand_regex_vars($tok->{value});
                } else {
                    ($tok->{path}, $tok->{subpath})
                        = resolve_conf_path($tok->{value});
                }
            }

            push @entries, {
                kind => 'rule',
                verb => $sign eq '+' ? 'allow' : 'deny',
                cats => \%cats,
                tokens => \@t,
                skipped => $skipped,
                src => $src,
                text => $line,
            };
            return 1;
        };

        unless ($ok) {
            my $msg = $@;
            chomp $msg;
            push @$errors, "$src: $msg";
        }
    }
    return @entries;
}

#-------------------------------------------------------------------------------
# Rule building and evaluation

sub filter_match {
    my ($f, $p) = @_;
    my $v = $f->{value};
    return 1 if $f->{type} eq 'all';
    return $p eq $v if $f->{type} eq 'literal';
    if ($f->{type} eq 'subpath') {
        return 1 if $v eq '/';
        return $p eq $v || index($p, "$v/") == 0;
    }
    if ($f->{type} eq 'regex') {
        my $r = eval { $p =~ /$v/ };
        return $r ? 1 : 0;
    }
    return 0;
}

# Last matching rule wins; returns (verb, rule-or-undef)
sub decide {
    my ($rules, $cat, $path) = @_;
    my ($verb, $who) = ('deny', undef);
    foreach my $r (@$rules) {
        next unless $r->{cats}{$cat};
        next unless grep { filter_match($_, $path) } @{$r->{filters}};
        ($verb, $who) = ($r->{verb}, $r);
    }
    return ($verb, $who);
}

# Turn a parsed rule's tokens into sandbox filters
sub make_filters {
    my ($tokens) = @_;
    my @filters;

    foreach my $tok (@$tokens) {
        if ($tok->{type} eq 'regex') {
            push @filters, { type => 'regex', value => $_ }
                foreach apply_regex_rewrites($tok->{value});
            next;
        }

        my $path = $tok->{path};
        my ($type, $note);
        if ($tok->{literal}) {
            $type = 'literal';
        } elsif ($tok->{subpath} || -d $path) {
            $type = 'subpath';
        } elsif (-e $path) {
            $type = 'literal';
        } else {
            $type = 'subpath';
            $note = 'does not exist';
        }

        foreach my $p (apply_rewrites(expand_links($path))) {
            push @filters, { type => $type, value => $p, note => $note };
        }
    }
    return @filters;
}

# Build the full, ordered rule list
sub build_rules {
    my @errors;
    @rewrites = ();

    my @sys = parse_conf(system_conf(), 'system', \@errors);
    my @cwd = parse_conf('+rwxm ${CWD}', 'cwd', \@errors);

    my @user;
    if (-e $conf_file) {
        open(my $fh, '<', $conf_file) or die "Cannot open $conf_file: $!\n";
        my $text = do { local $/; <$fh> };
        close($fh);
        @user = parse_conf($text, 'conf', \@errors);
    } else {
        print STDERR "Note: $conf_file not found; using system rules only\n";
    }

    my @forced = parse_conf(forced_conf(), 'forced', \@errors);

    if (@errors) {
        print STDERR "$_\n" foreach @errors;
        die "Error: bad configuration, profile not generated\n";
    }

    my @rules = ({
        kind => 'rule',
        verb => 'allow',
        cats => { M => 1 },
        filters => [{ type => 'all' }],
        src => 'system',
        text => 'stat anything not denied below',
    });

    foreach my $e (@sys, @cwd, @user, @forced) {
        if ($e->{kind} ne 'rule') {
            push @rules, $e;
            next;
        }

        $e->{filters} = [make_filters($e->{tokens})];

        # Parent directories that are now hidden need stat access, or
        # realpath(), getcwd() and friends fail on the way down
        my @parents;
        if ($e->{verb} eq 'allow') {
            my %seen;
            foreach my $f (@{$e->{filters}}) {
                next if $f->{type} eq 'regex';
                my $p = $f->{value};
                while ($p ne '/') {
                    $p = dirname($p);
                    next if $seen{$p}++;
                    my ($verb) = decide(\@rules, 'M', $p);
                    push @parents, $p if $verb eq 'deny';
                }
            }
        }

        push @rules, $e;

        if (@parents) {
            push @rules, {
                kind => 'rule',
                verb => 'allow',
                cats => { M => 1 },
                filters => [map { { type => 'literal', value => $_ } }
                            reverse @parents],
                src => $e->{src},
                text => 'parent directories (stat only)',
            };
        }
    }
    return @rules;
}

# The implicit system rules, in conf syntax
sub system_conf {
    my @opt;
    foreach my $p ("/Applications/Xcode.app", "/Library/Developer",
                   "/opt/homebrew", "/opt/local", "/usr/local/Homebrew",
                   "/nix/store", "~/.local") {
        my ($abs) = resolve_conf_path($p);
        push @opt, conf_quote($p) if -d $abs;
    }

    # Wherever the claude binary really lives
    my $claude_dir;
    foreach my $dir (split /:/, $ENV{PATH} || '') {
        next unless $dir =~ m|^/| && -x "$dir/claude";
        my @chain = expand_links(norm_path("$dir/claude"));
        $claude_dir = dirname($chain[-1]);
        last;
    }

    my $text = <<'EOF';

# ---------- basic dir traversal -----------------------------------
+r =/ =/private

# ---------- hide everything in ~ unless allowed below -------------
-a ~

# ---------- system runtime ----------------------------------------
+rxm /System /usr /bin /sbin /Library/Frameworks /opt /private/etc /var/db/dyld
EOF
    $text .= "+rxm @opt\n" if @opt;
    $text .= "\n# ---------- claude binary -----------------------------------------\n"
        . "+rxm " . conf_quote("$claude_dir/") . "\n" if $claude_dir;
    $text .= <<'EOF';

# ---------- temp dirs ---------------------------------------------
+rw /tmp/ ${TMPDIR}/../

# ---------- Claude config & token files ---------------------------
+rw ~/.claude/ r"^${HOME}/\.claude\.json(\..*)?$"

# ---------- time-zone & prefs (RO) --------------------------------
+r /private/var/db/timezone/ /Library/Preferences/ ~/Library/Preferences/

# ---------- /dev access -------------------------------------------
+r =/dev
+rw r"^/dev/(tty.*|null|zero|dtracehelper)"
+i =/dev/dtracehelper r"^/dev/tty.*"
+rwi /dev/fd/ =/dev/stdin =/dev/stdout =/dev/stderr
EOF
    return $text;
}

# Denies that come after the user's conf, so it cannot undo them
sub forced_conf {
    return "\n# ---------- forced denies (not overridable) ----------------------\n"
        . "-w =" . conf_quote($conf_file) . "\n"
        . "-a ~/.ssh ~/.gnupg\n";
}

# Quote a path for conf syntax, if needed
sub conf_quote {
    my ($p) = @_;
    return $p unless $p =~ /[\s"\\#\$]/;
    $p =~ s/(["\\\$])/\\$1/g;
    return "\"$p\"";
}

#-------------------------------------------------------------------------------
# Profile generation

# Quote a string for SBPL
sub sb_str {
    my ($s) = @_;
    $s =~ s/(["\\])/\\$1/g;
    return "\"$s\"";
}

sub emit_rule {
    my ($r) = @_;
    my @ops;
    foreach my $c (split //, $CATS) {
        push @ops, $CAT_OP{$c} if $r->{cats}{$c};
    }
    push @ops, $CAT_OP{M} if $r->{cats}{M} && !$r->{cats}{r};

    my $out = ";; $r->{src}: $r->{text}\n";
    $out .= ";;   skipped: $_\n" foreach @{$r->{skipped} || []};
    my @f = grep { $_->{type} ne 'all' } @{$r->{filters}};
    unless (@f) {
        return $out . "($r->{verb} @ops)\n";
    }

    if ($one_per_line) {
        foreach my $f (@f) {
            $out .= "($r->{verb} @ops ($f->{type} " . sb_str($f->{value}) . "))";
            $out .= "    ;; $f->{note}" if $f->{note};
            $out .= "\n";
        }
        return $out;
    }

    $out .= "($r->{verb} @ops\n";
    foreach my $f (@f) {
        $out .= "  ($f->{type} " . sb_str($f->{value}) . ")";
        $out .= "    ;; $f->{note}" if $f->{note};
        $out .= "\n";
    }
    return $out . ")\n";
}

sub generate_profile {
    my @rules = build_rules();
    my $now = strftime("%Y-%m-%d %H:%M:%S", localtime);

    my $out = <<EOF;
;; ------------------------------------------------------------------
;;  Claude Code macOS sandbox profile (autogenerated)
;;  generated: $now
;;  workspace: $project_dir
;;  config:    $conf_file
;; ------------------------------------------------------------------
(version 1)
(deny default)

(allow sysctl-read)
EOF

    foreach my $r (@rules) {
        if ($r->{kind} eq 'blank') {
            $out .= "\n";
        } elsif ($r->{kind} eq 'comment') {
            $out .= ";; $r->{text}\n";
        } else {
            $out .= emit_rule($r);
        }
    }

    $out .= <<'EOF';

;; ---------- mach-lookup services ---------------------------------
(allow mach-lookup
  (global-name "com.apple.system.opendirectoryd.libinfo")
  (global-name "com.apple.SystemConfiguration.DNSConfiguration")
  (global-name "com.apple.coreservices.launchservicesd")
  (global-name "com.apple.CoreServices.coreservicesd")
  (global-name "com.apple.system.notification_center")
  (global-name "com.apple.logd")
  (global-name "com.apple.diagnosticd")
  (global-name "com.apple.lsd.mapdb")
  (global-name "com.apple.lsd.modifydb")
  (global-name "com.apple.coreservices.quarantine-resolver")
)

;; ---------- Launch Services needed by /usr/bin/open --------------
(allow mach-lookup
  (regex "^com\\.apple\\.lsd(\\..*)?$")   ;; lsd + its mapdb / modifydb helpers
)

;; ---------- Notification Center shared-memory (RO) ---------------
(allow ipc-posix-shm-read-data
  (ipc-posix-name "apple.shm.notification_center")
)

;; ---------- networking --------------------------------------------
(allow network*)

(allow process-fork)
(allow lsopen)
EOF

    open(my $fh, '>', $profile_file) or die "Cannot create profile file: $!";
    print $fh $out;
    close($fh);
}

#-------------------------------------------------------------------------------
# check: explain the decision for each path

sub check_paths {
    my (@paths) = @_;
    unless (@paths) {
        print STDERR "Usage: claude-sandbox check path...\n";
        exit 1;
    }

    my @rules = grep { $_->{kind} eq 'rule' } build_rules();

    foreach my $arg (@paths) {
        my $p = $arg =~ m|^/| ? $arg : getcwd() . "/$arg";
        $p = norm_path($p);
        my @chain = expand_links($p);
        my $real = $chain[-1];

        print "$arg\n";
        print "  resolves to $real\n" if $real ne $p;
        foreach my $c ('M', split //, $CATS) {
            my ($verb, $who) = decide(\@rules, $c, $real);
            my $why = $who ? "$who->{src}: $who->{text}" : "(default)";
            printf "  %-8s %-5s  %s\n", $CAT_NAME{$c}, $verb, $why;
        }
    }
}

sub run_claude {
    my @args = @_;
    generate_profile();

    print "- Running Claude Code sandboxed in:  $project_dir\n";
    print "- Profile: $profile_file\n";
    print "- Config:  $conf_file\n";

    my $dir = $ENV{HOME} . "/.claude/logs";
    mkdir($dir);
    my $time = strftime("%Y%m%d-%H%M.%S", localtime);

    my $file = $time . ".log";
    my $path = $dir . "/" . $file;

    # Native exec replacement (replaces current perl thread)
    exec("script", $path, "sandbox-exec", "-f", $profile_file, @args)
        or die "Failed to execute sandbox-exec: $!";
}
