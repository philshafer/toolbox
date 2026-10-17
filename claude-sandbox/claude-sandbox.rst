==============
claude-sandbox
==============

------------------------------------------------
run Claude Code inside a tighter macOS sandbox
------------------------------------------------

.. contents::
   :local:
   :depth: 2

Synopsis
========

::

    claude-sandbox [-f file] [-x] [command] [args...]

    claude-sandbox                # generate profile + run claude (default)
    claude-sandbox run [...args]  # same, pass args through to claude
    claude-sandbox shell          # generate profile + run $SHELL
    claude-sandbox generate       # build the profile only
    claude-sandbox profile        # print last-generated profile path
    claude-sandbox check path...  # report which rule decides each access
    claude-sandbox help           # short usage text

Description
===========

``claude-sandbox`` runs Claude Code under ``sandbox-exec(1)`` with a
profile that keeps it out of most of your home directory.  By default
Claude can:

- read and run the system (``/System``, ``/usr``, ``/bin``, Homebrew,
  Xcode, and so on)
- read, write, and run anything in the directory you started it from
  (the *workspace*)
- use its own files (``~/.claude``, ``~/.claude.json``), the temp
  directories, and the usual ``/dev`` entries
- use the network

Everything else under ``~`` is hidden.  Claude cannot read it, write
it, or even see that it exists.  You open up what you need, such as
your editor settings, your tools, and other source trees, in a small
config file, ``~/.claude-sandbox.conf``.

Each time it runs, ``claude-sandbox`` builds a fresh profile from the
built-in system rules and your config file, writes it to
``$TMPDIR/claude-sandbox-<dir>-<hash>.sb``, and then runs::

    script ~/.claude/logs/<date>.log sandbox-exec -f <profile> claude ...

so each session is also logged under ``~/.claude/logs``.

Commands
========

``run [args...]``
    Generate the profile and run ``claude`` inside the sandbox, with
    *args* passed through.  This is the default.  Any word that is not
    a known command is passed through to ``claude`` too, so
    ``claude-sandbox --resume`` works.

``shell``
    Generate the profile and run ``$SHELL`` inside the sandbox instead
    of Claude.  This is the easiest way to poke at the sandbox by hand:
    try ``ls ~``, ``cat ~/.ssh/id_rsa``, or ``touch`` in a directory you
    allowed.

``generate``
    Build the profile and print its path, without running anything.
    Read the profile to see exactly what will be allowed.

``profile``
    Print the path the profile would be written to for the current
    directory.

``check path...``
    Show, for each *path*, whether each kind of access is allowed or
    denied, and which rule decided it.  See `Checking a path`_.

``help``
    Print a short usage summary.  ``-h`` and ``--help`` work too.

Options
=======

Options come before the command.

``-f file``
    Read rules from *file* instead of ``~/.claude-sandbox.conf``.
    This is useful for trying out a new config before you install it.
    If ``-f`` is given more than once, the last one wins.

``-x``
    Extra-verbose profile: write one rule line per path instead of one
    rule per config line.  For example, this::

        (allow file-read* process-exec
          (subpath "/Users/me/bin")
          (subpath "/Users/me/src")
        )

    becomes::

        (allow file-read* process-exec (subpath "/Users/me/bin"))
        (allow file-read* process-exec (subpath "/Users/me/src"))

The config file
===============

``~/.claude-sandbox.conf`` holds your rules, one per line.  It is
optional; without it you get only the system rules, and
``claude-sandbox`` prints a note saying so.  A commented example is in
``claude-sandbox.conf.example``.

A small config might look like this::

    # ~/work is a symlink onto another volume
    @rewrite work/ /Volumes/case/work/

    # editor and git settings (read-only)
    .exrc .vimrc .gitconfig

    # my tools (read + run)
    +rx bin src

    # other trees Claude may change
    +rwx work/root patches/claude

    # a build output dir, if I've set one
    +rwx ?${SB_DESTDIR}

    # but not the secrets in it
    - work/root/secrets

Rule lines
----------

Each rule line looks like this::

    [+|-][letters] path [path ...]

``+`` allows and ``-`` denies.  The letters say which kinds of access
the rule covers:

====== ========================= ===================================
Letter Access                    sandbox operation
====== ========================= ===================================
``r``  read files, list dirs     ``file-read*``
``w``  create, change, delete    ``file-write*``
``x``  run programs              ``process-exec``
``i``  ioctl (terminals, etc.)   ``file-ioctl``
``m``  map as executable code    ``file-map-executable``
``a``  all of the above
====== ========================= ===================================

A few shortcuts:

- An allow always includes read.  ``+w foo`` means ``+rw foo``, and
  ``+`` alone means ``+r``.
- A line with no ``+`` or ``-`` at all is ``+r``.  ``.vimrc`` alone on
  a line makes it readable.
- A deny covers only the letters you give.  ``-w foo`` makes ``foo``
  read-only and leaves read alone.
- ``-`` alone means ``-a``, which denies everything, including
  seeing that the path exists.

``m`` is needed for shared libraries, Python extension modules and
other code that is loaded rather than run, so tool and cache
directories usually want ``+rxm``.

You can list as many paths as you like on one line; they all get the
same access.

Comments and blank lines
------------------------

A line starting with ``#`` is a comment.  Comments and blank lines are
copied into the generated profile (as ``;;`` comments), so the profile
keeps the shape of your config and is easy to read.  A ``#`` that
starts a word, after the paths on a rule line, also starts a comment::

    +rx bin     # my scripts

Paths
-----

Paths are relative to your home directory unless they say otherwise:

==================== ================================================
Path                 Means
==================== ================================================
``foo/bar``          ``$HOME/foo/bar``
``/foo/bar``         ``/foo/bar`` (absolute)
``~`` or ``~/foo``   your home directory, or a path under it
``${HOME}/foo``      same as ``~/foo``
``${CWD}/foo``       ``foo`` in the workspace (where you started)
``${TMPDIR}/foo``    ``foo`` in your temp directory
==================== ================================================

``~user`` is not supported.  See `Variables`_ for ``${NAME}``.

**Directories and files.**  A directory becomes a ``subpath`` rule,
which covers the directory and everything below it.  A file becomes a
``literal`` rule, which covers just that file.  ``claude-sandbox``
looks at the disk to decide, so a path that does not exist yet is
treated as a directory, and the profile notes that it does not exist.
You can say which you mean:

- a trailing ``/`` (``build/``) always means a directory
- a leading ``=`` (``=.netrc``) always means exactly that one path

**Spaces and quotes.**  Put a path with spaces in double quotes.
Inside the quotes, a backslash escapes the next character, and
``\$`` is a real ``$`` rather than the start of a variable::

    +r "Library/Application Support/MyApp" ="My \"odd\" file"
    +r "Library/Price\$List"

**Regular expressions.**  ``r"..."`` is a regular expression, matched
against the full path, as ``sandbox-exec`` does.  Write it the usual
way; you do not need to double the backslashes.  A regex is not
relative to anything, so anchor it with ``^`` and an absolute path.
``${NAME}`` is substituted inside a regex, with any special characters
in the value escaped.  A ``$`` that is not followed by ``{`` is the
usual end-of-path anchor::

    +rw r"^${HOME}/\.claude\.json(\..*)?$"

**Symlinks.**  The sandbox checks the real path, after symlinks are
resolved, so ``claude-sandbox`` follows every symlink in a path and
writes a rule for each step of the chain.  If ``~/work`` is a link to
``/Volumes/case/work``, then ``+rwx work/root`` covers both
``~/work/root`` and ``/Volumes/case/work/root``.

Variables
---------

``${NAME}`` anywhere in a path (or a regex) is replaced by the value
of the environment variable ``NAME``.  The braces are required: a
``$`` in a path must start a ``${NAME}``, so ``$FOO/bar`` is an error
that suggests ``${FOO}/bar`` instead.

Three names are special:

``${CWD}``
    The workspace: the directory you started ``claude-sandbox`` from,
    with symlinks resolved.  This is not the environment's ``$PWD``,
    which may still hold a symlinked path.

``${HOME}``, ``${TMPDIR}``
    The usual environment variables, but cleaned up (no ``//`` or
    trailing ``/``), so ``${TMPDIR}/foo`` is a tidy path.  If
    ``TMPDIR`` is not set, ``/tmp`` is used.

If any other variable is unset or empty, that is an error and no
profile is made, because a missing value would otherwise turn into a
much broader rule than you meant (``+rwx ${FOO}/out`` would become
``+rwx /out``, or ``+rwx ${FOO}`` would become ``+rwx ~``).

Optional paths
--------------

Put ``?`` in front of a path to make it optional.  If any
``${NAME}`` it uses is unset or empty, that path is quietly left out
instead of causing an error::

    +rwx ?${SB_DESTDIR}
    +r ?${SB_PROJ}/build .exrc

The other paths on the line are still used.  Paths that were left out
are listed in the profile::

    ;; conf:2: +rwx ?${SB_DESTDIR}
    ;;   skipped: ?${SB_DESTDIR} (SB_DESTDIR not set)

``?`` works with every path form (``?"quoted"``, ``?=literal``,
``?r"regex"``), on deny lines, and with ``@rewrite``.  A ``?`` on a
path with no environment variables (or only ``${CWD}``, ``${HOME}``
and ``${TMPDIR}``, which are always set) does nothing.  A malformed
``${`` or a ``$`` without braces is still an error.

@rewrite
--------

::

    @rewrite from to

``@rewrite`` tells ``claude-sandbox`` that two directory trees are the
same thing.  Every rule path under *from* also gets a copy under *to*,
and the other way around.  Rewrites apply to all rules, wherever the
``@rewrite`` line is in the file.

This is for places where symlinks are not enough: the path you write
is not itself a symlink, but the files are reached under another name.
A common case is a path that Claude sees through one name but that the
system resolves to another, such as a volume mounted under
``/Volumes``::

    @rewrite work/ /Volumes/case/work/

Both sides must be plain paths (no ``=`` or regex).  A regex that
starts with ``^`` and one side of a rewrite is copied for the other
side too.

Errors
------

Every problem in the config file is reported with its line number,
and no profile is made until they are all fixed::

    conf:7: unknown option letter "q"
    conf:8: use "${CWD}" instead of "$CWD"
    conf:9: environment variable "SB_DESTDIR" is not set
    Error: bad configuration, profile not generated

How the rules combine
=====================

The profile is a list of rules, and **the last rule that matches a
path wins**.  The rules come in this order:

1. **System rules.**  Read and run the system, then ``-a ~`` to hide
   all of your home directory, then allows for the few things under
   ``~`` that Claude needs (``~/.claude``, ``~/.claude.json``,
   ``~/Library/Preferences``, ``~/.local`` and the directory holding
   the ``claude`` binary).
2. **The workspace**: ``+rwxm ${CWD}``.
3. **Your config file**, top to bottom.
4. **Forced denies**, which your config cannot undo:

   - ``-w`` on the config file itself, so Claude cannot change its own
     sandbox
   - ``-a ~/.ssh ~/.gnupg``

Because everything under ``~`` starts out denied, most configs need
only allows.  Use a deny to take something back from a broader allow
above it::

    +rwx work
    -w work/release        # readable, but not writable
    - work/root/secrets    # not even visible

Order matters: a deny placed *before* the allow it should restrict is
overridden by that allow.

**Parent directories.**  Hiding ``~`` also hides the directories on
the way down to anything you allow, and many programs (``realpath``,
``getcwd``, shells) fail if they cannot see them.  So after each allow,
``claude-sandbox`` adds a stat-only rule for any parent directory that
would otherwise be hidden.  Claude can see that ``~/work`` exists and
step through it to ``~/work/root``, but cannot list or read the other
things in ``~/work``.  These show up in the profile as::

    ;; conf:6: parent directories (stat only)
    (allow file-read-metadata
      (literal "/Users/me/work")
    )

**Not configurable.**  Networking is always allowed, as are a fixed
set of system services (DNS, Launch Services, notifications, logging)
needed by Claude and ``/usr/bin/open``.

Checking a path
===============

``claude-sandbox check`` answers "why can't Claude see this?" without
starting a session.  For each path it prints the decision for each
kind of access, and the rule that made it::

    $ claude-sandbox check ~/.exrc ~/work/root/foo
    /Users/me/.exrc
      stat     allow  conf:4: .exrc .vimrc
      read     allow  conf:4: .exrc .vimrc
      write    deny   system:6: -a ~
      exec     deny   system:6: -a ~
      ioctl    deny   system:6: -a ~
      map-exec deny   system:6: -a ~
    /Users/me/work/root/foo
      resolves to /Volumes/case/work/root/foo
      stat     allow  conf:6: +rwx work/root patches/claude
      read     allow  conf:6: +rwx work/root patches/claude
      write    allow  conf:6: +rwx work/root patches/claude
      exec     allow  conf:6: +rwx work/root patches/claude
      ioctl    deny   (default)
      map-exec deny   (default)

``conf:N`` is line *N* of your config file; ``system``, ``cwd`` and
``forced`` are the built-in rules (see ``claude-sandbox generate`` for
their line numbers).  ``(default)`` means no rule matched, and the
profile's ``(deny default)`` applies.  Relative paths are taken from
the current directory, as in the shell, not from ``$HOME``.

``check`` works out the answer from the rules itself rather than
asking the kernel, so for anything subtle, confirm with
``claude-sandbox shell``.

Files
=====

``~/.claude-sandbox.conf``
    Your rules.  Claude can read it but not change it.

``$TMPDIR/claude-sandbox-<dir>-<hash>.sb``
    The generated profile for workspace *dir*.  It is rebuilt on every
    run, so edit the config, not the profile.

``~/.claude/logs/<date>.log``
    A ``script(1)`` log of each session.

Notes
=====

- ``sandbox-exec`` is deprecated by Apple but still works on current
  macOS releases.
- The sandbox applies to Claude and everything it runs.  If a tool
  fails in odd ways inside the sandbox (a missing config file, "Operation
  not permitted"), run ``claude-sandbox check`` on the path it needs,
  or try the command in ``claude-sandbox shell``.
- A sandbox cannot be started from inside another one, so
  ``claude-sandbox`` will not work from within a sandboxed session.

See also
========

``sandbox-exec(1)``, ``script(1)``, ``claude-sandbox.conf.example``
