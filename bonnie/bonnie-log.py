#!/usr/bin/env python3
#
# Converts a `script`(1) typescript capture of a Claude Code terminal
# session into a readable rST transcript, alternating "Input:" (what
# the user typed) and "Output:" (what Claude printed) sections, with
# terminal colors/attributes encoded as Sphinx/docutils color roles
# (":rolename:`text`") plus a companion .css file defining them.
#
# Usage: bin/bonnie-log.py [LOGFILE] [OUTBASE]
#   LOGFILE defaults to build/work.log (relative to the repo root)
#   OUTBASE defaults to LOGFILE with its extension stripped; output is
#   written to OUTBASE.rst and OUTBASE.css
#
import colorsys
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_TOP = os.path.dirname(_HERE)
sys.path.insert(0, os.path.join(_TOP, "build", "pydeps"))

MARKER_PREFIXES = ("⏺", "✻", "※", "❯")  # Y, *, *, >
INPUT_PROMPT = "❯ "  # the literal "> " prompt glyph + space

# Box-drawing / separator glyphs used purely for framing (permission
# dialogs, horizontal rules); lines made up of nothing else carry no
# information and are dropped.
_BORDER_RE = re.compile(r"^[─-╿\s]+$")

CONTINUATION_STOP_RE = re.compile(
    r"^(⏺|✻|※|❯|  ⏿|Searched for |Read |Ran )"
)


def load_screen(data, columns=220, lines=50, history=200000):
    import pyte
    from pyte import HistoryScreen
    from pyte.streams import ByteStream, Stream

    # pyte 0.8.2 has no CSI dispatch entries for Scroll Up / Scroll Down
    # (CSI n S / CSI n T). This Claude Code TUI build relies on exactly
    # those sequences -- not newline scrolling -- to commit finished
    # screen content, so without this patch almost everything is lost.
    Stream.csi["S"] = "scroll_up_history"
    Stream.csi["T"] = "scroll_down_history"

    class LogScreen(HistoryScreen):
        def report_device_status(self, *args, **kwargs):
            pass  # DECRQM/DSR queries: no real terminal to answer them

        def scroll_up_history(self, count=None):
            count = count or 1
            top, bottom = self.margins or (0, self.lines - 1)
            for _ in range(count):
                self.history.top.append(self.buffer[top])
                for y in range(top, bottom):
                    self.buffer[y] = self.buffer[y + 1]
                self.buffer.pop(bottom, None)
            self.dirty.update(range(self.lines))

        def scroll_down_history(self, count=None):
            count = count or 1
            top, bottom = self.margins or (0, self.lines - 1)
            for _ in range(count):
                self.history.bottom.append(self.buffer[bottom])
                for y in range(bottom, top, -1):
                    self.buffer[y] = self.buffer[y - 1]
                self.buffer.pop(top, None)
            self.dirty.update(range(self.lines))

    screen = LogScreen(columns, lines, history=history)
    stream = ByteStream(screen)
    stream.feed(data)
    return screen


def strip_script_wrapper(data):
    # Drop the `script` header line ("Script started on ...") and, if
    # present, its trailing "Script done on ..." line.
    first_nl = data.find(b"\n")
    body = data[first_nl + 1:] if first_nl != -1 else data
    tail = body.rfind(b"\nScript done on ")
    if tail != -1:
        body = body[:tail]
    return body


DEFAULT_ATTRS = "default"

# pyte's named ANSI colors, mapped to their standard xterm RGB values so
# background names can be lightness-adjusted the same way as truecolor
# hex backgrounds. "bfightmagenta" is a typo in this pyte version's own
# BG_AIXTERM table, kept here so that specific capture still resolves.
NAMED_COLOR_HEX = {
    "black": "000000", "red": "cd0000", "green": "00cd00", "brown": "cdcd00",
    "blue": "1a1aee", "magenta": "cd00cd", "cyan": "00cdcd", "white": "e5e5e5",
    "brightblack": "7f7f7f", "brightred": "ff0000", "brightgreen": "00ff00",
    "brightbrown": "ffff00", "brightblue": "5c5cff", "brightmagenta": "ff00ff",
    "bfightmagenta": "ff00ff", "brightcyan": "00ffff", "brightwhite": "ffffff",
}

# Text is always rendered in black (see RoleRegistry.css), so any
# background darker than this would make it unreadable (this is how the
# reported "light blue block, white text disappears" bug happened: the
# terminal's own fg/bg pairing was readable on a dark terminal background
# but not once fg is dropped in favor of a fixed black). Colors are
# lightened toward this floor in HSL space, preserving hue, rather than
# picked from a fixed palette, so the original color family still shows.
MIN_BG_LIGHTNESS = 0.80


def safe_bg_hex(bg):
    hexval = NAMED_COLOR_HEX.get(bg, bg)
    r, g, b = (int(hexval[i:i + 2], 16) / 255.0 for i in (0, 2, 4))
    h, l, s = colorsys.rgb_to_hls(r, g, b)
    if l >= MIN_BG_LIGHTNESS:
        return hexval
    r, g, b = colorsys.hls_to_rgb(h, MIN_BG_LIGHTNESS, s)
    return "".join("%02x" % round(c * 255) for c in (r, g, b))


def cell_char(char):
    # Each screen cell is exactly one terminal column; a real tab byte
    # is dispatched to the screen's cursor-advancing tab() method, not
    # drawn into a cell, so this should never fire on normal input. It's
    # a defensive expansion for any stray literal tab that lands in a
    # cell anyway (e.g. from a capture this replay logic doesn't fully
    # model) -- a single space keeps that column's width correct, which
    # a raw tab character would otherwise blow up when rendered in a
    # monospace block.
    return " " if char.data == "\t" else char.data


def attrs_of(char):
    # Only background is tracked. fg is ignored entirely -- text is
    # always rendered black (see RoleRegistry.css), both because this
    # capture has stretches where an underline (or other modifier) SGR
    # bleeds a stale fg across unrelated later spans, and because a
    # fixed black-on-lightened-bg pairing is always readable, which the
    # terminal's own fg/bg pairing is not once lifted onto a page
    # background (that mismatch is the "light blue block, white text
    # disappears" bug).
    return char.bg


class RoleRegistry:
    def __init__(self):
        self._names = {}
        self._order = []

    def name_for(self, bg):
        name = self._names.get(bg)
        if name is not None:
            return name
        name = "c-bg-" + bg if bg != "default" else "c-plain"
        self._names[bg] = name
        self._order.append((name, bg))
        return name

    def names(self):
        return [name for name, _ in self._order]

    def css(self):
        lines = []
        for name, bg in self._order:
            decls = ["color: black;"]
            if bg != "default":
                decls.append("background-color: #%s;" % safe_bg_hex(bg))
            lines.append(".%s {\n    %s\n}" % (name, "\n    ".join(decls)))
        return "\n".join(lines) + "\n" if lines else ""


def render_line(line_map, cols, roles):
    """Returns (plain, rendered) for one screen/history line, or None
    for a pure border/rule line. `plain` has no role markup and is
    used for all structural pattern-matching (prompt/marker
    detection); `rendered` has the rst color-role markup and is what
    actually gets emitted."""
    # Border/rule lines (permission-dialog framing, horizontal rules)
    # carry no information; detect them from the plain characters
    # before any role markup is added, since the regex can't see
    # through literal rst markup once it's inserted.
    plain = "".join(cell_char(line_map[x]) for x in range(cols)).rstrip()
    if is_border_line(plain):
        return None

    spans = []
    cur_text = []
    cur_attrs = None
    for x in range(cols):
        ch = line_map[x]
        a = attrs_of(ch)
        if a != cur_attrs:
            if cur_text:
                spans.append(("".join(cur_text), cur_attrs))
            cur_text = [cell_char(ch)]
            cur_attrs = a
        else:
            cur_text.append(cell_char(ch))
    if cur_text:
        spans.append(("".join(cur_text), cur_attrs))

    # Trailing whitespace carries no visible information even when it's
    # colored (e.g. a diff line's background tint extended to the
    # terminal's right margin), so it's dropped regardless of attrs --
    # including trailing whitespace trapped inside the last non-blank
    # span (e.g. "...every   " where the background tint runs past the
    # last real character).
    while spans:
        text, a = spans[-1]
        if text.strip() == "":
            spans.pop()
            continue
        stripped = text.rstrip()
        if stripped != text:
            spans[-1] = (stripped, a)
        break

    out = []
    for text, a in spans:
        if text == "":
            continue
        # Pure whitespace mid-line: coloring it would be invisible but
        # still costly markup, so render it plain.
        if a == DEFAULT_ATTRS or text.strip() == "":
            out.append(text)
        else:
            # docutils requires a role's start-string be immediately
            # followed by non-whitespace and its end-string immediately
            # preceded by non-whitespace -- but a span's colored run
            # commonly starts or ends with padding spaces that share its
            # attrs (e.g. indentation merged with the text that follows
            # it). Peel that whitespace out of the role and emit it
            # plain on either side instead of letting it break parsing.
            lead_len = len(text) - len(text.lstrip())
            trail_len = len(text) - len(text.rstrip())
            lead = text[:lead_len]
            trail = text[len(text) - trail_len:] if trail_len else ""
            core = text[lead_len:len(text) - trail_len] if trail_len else text[lead_len:]
            if lead:
                out.append(lead)
            esc = core.replace("`", r"\`")
            out.append(":%s:`%s`" % (roles.name_for(a), esc))
            if trail:
                out.append(trail)

    # Adjacent role-wrapped spans butt up against each other with no
    # whitespace (one color run ending exactly where the next begins, as
    # happens constantly in syntax-highlighted code). docutils only
    # recognizes an interpreted-text start-string when it's preceded by
    # whitespace or a small set of punctuation -- a bare backtick isn't
    # in that set -- so "...`:rolename:`..." with no gap silently fails
    # to parse as markup and prints as literal text instead. "\ " is
    # docutils' standard escaped-whitespace idiom: it satisfies the
    # adjacency rule but contributes nothing to the rendered output.
    rendered = ""
    for piece in out:
        if rendered and piece and not rendered[-1].isspace() and not piece[0].isspace():
            rendered += "\\ "
        rendered += piece

    has_bg = any(a != "default" for _, a in spans)
    return plain, rendered.rstrip(), has_bg


def is_border_line(line):
    return bool(line) and bool(_BORDER_RE.match(line))


def reconstruct_lines(screen, roles):
    history_lines = list(screen.history.top)
    current_lines = [screen.buffer[y] for y in range(screen.lines)]
    all_lines = history_lines + current_lines

    rendered = [render_line(l, screen.columns, roles) for l in all_lines]
    rendered = [l for l in rendered if l is not None]

    while rendered and rendered[0][0] == "":
        rendered.pop(0)
    while rendered and rendered[-1][0] == "":
        rendered.pop()

    collapsed = []
    prev_blank = False
    for plain, text, has_bg in rendered:
        blank = (plain == "")
        if blank and prev_blank:
            continue
        collapsed.append((plain, text, has_bg))
        prev_blank = blank

    # The TUI pads a background-tinted block (a diff hunk) with a blank,
    # uncolored terminal row directly above and below it for visual
    # breathing room in a live terminal. Replayed into a transcript, that
    # padding is just a stray blank line hugging the code block, so drop
    # a blank line that borders a bg-colored line on either side.
    final = []
    for i, (plain, text, has_bg) in enumerate(collapsed):
        if plain == "":
            prev_bg = collapsed[i - 1][2] if i > 0 else False
            next_bg = collapsed[i + 1][2] if i + 1 < len(collapsed) else False
            if prev_bg or next_bg:
                continue
        final.append((plain, text))
    return final


def segment_turns(lines):
    """Split reconstructed (plain, rendered) line pairs into a leading
    preamble (Output-only, present when the capture starts mid-session)
    followed by (input_text, output_lines) turn pairs. All structural
    classification (prompt lines, continuation lines) is decided from
    the plain text; `output_lines` carries the color-rendered text."""
    turns = []
    preamble = []
    i = 0
    n = len(lines)

    def is_prompt(pair):
        return pair[0].startswith(INPUT_PROMPT)

    while i < n and not is_prompt(lines[i]):
        preamble.append(lines[i][1])
        i += 1

    while i < n:
        assert is_prompt(lines[i])
        plain, _ = lines[i]
        input_lines = [plain[len(INPUT_PROMPT):]]
        i += 1
        while i < n and lines[i][0] != "" and not CONTINUATION_STOP_RE.match(lines[i][0]):
            plain = lines[i][0]
            input_lines.append(plain[2:] if plain.startswith("  ") else plain)
            i += 1
        input_text = " ".join(s.strip() for s in input_lines if s.strip())

        output_lines = []
        while i < n and not is_prompt(lines[i]):
            output_lines.append(lines[i][1])
            i += 1

        while output_lines and output_lines[0] == "":
            output_lines.pop(0)
        while output_lines and output_lines[-1] == "":
            output_lines.pop()

        turns.append((input_text, output_lines))

    return preamble, turns


def parsed_literal(lines):
    """Wraps terminal-output lines in a docutils `parsed-literal` block:
    whitespace is preserved exactly (unlike a plain paragraph, which
    reflows and chokes on irregular leading indentation -- the terminal
    capture mixes 0-, 1- and 5-space indents line to line), while
    ":rolename:`text`" markup inside it is still processed, unlike a
    plain `::` literal block."""
    if not lines:
        return []
    out = [".. parsed-literal::", ""]
    out.extend("   " + line if line else "" for line in lines)
    return out


def render_transcript(preamble, turns):
    out = []
    if preamble:
        out.append("Output:")
        out.append("-------")
        out.append("")
        out.extend(parsed_literal(preamble))
        out.append("")

    for input_text, output_lines in turns:
        out.append("Input:")
        out.append("------")
        out.append("")
        out.append(input_text)
        out.append("")
        out.append("Output:")
        out.append("-------")
        out.append("")
        out.extend(parsed_literal(output_lines))
        out.append("")

    return "\n".join(out).rstrip() + "\n"


def convert(data, stem):
    body = strip_script_wrapper(data)
    screen = load_screen(body)
    roles = RoleRegistry()
    lines = reconstruct_lines(screen, roles)
    preamble, turns = segment_turns(lines)
    transcript = render_transcript(preamble, turns)

    # Custom color roles must be declared before use; docutils' generic
    # `role` directive renders each as "<span class=rolename>", which is
    # exactly what the companion .css selects on.
    title = stem.replace("_", " ").replace("-", " ")
    header = [title, "=" * len(title), ""]
    header.extend(".. role:: %s" % name for name in roles.names())
    header.append("")

    rst = "\n".join(header) + "\n" + transcript
    return rst, roles.css()


CONF_PY = """\
project = %(project)r
master_doc = %(stem)r
extensions = []
html_css_files = [%(css)r]
html_static_path = ["_static"]
exclude_patterns = ["_build"]
"""


def write_sphinx_project(outdir, stem, rst, css):
    # The .css lives in its own "_static" subdir (never the project root)
    # so Sphinx's static-file copy doesn't recurse into _build/ on repeat
    # runs -- html_static_path pointing at "." grows without bound.
    staticdir = os.path.join(outdir, "_static")
    os.makedirs(staticdir, exist_ok=True)

    with open(os.path.join(outdir, stem + ".rst"), "w") as f:
        f.write(rst)
    with open(os.path.join(staticdir, stem + ".css"), "w") as f:
        f.write(css)
    with open(os.path.join(outdir, "conf.py"), "w") as f:
        f.write(CONF_PY % {
            "project": stem,
            "stem": stem,
            "css": stem + ".css",
        })


def main(argv):
    import getpass
    import subprocess

    logfile = argv[1] if len(argv) > 1 else os.path.join(_TOP, "build", "work.log")
    stem = os.path.splitext(os.path.basename(logfile))[0]

    user = os.environ.get("USER") or getpass.getuser()
    outdir = "/tmp/%s-bonnie" % user
    os.makedirs(outdir, exist_ok=True)
    builddir = os.path.join(outdir, "_build", "html")

    with open(logfile, "rb") as f:
        data = f.read()

    rst, css = convert(data, stem)
    write_sphinx_project(outdir, stem, rst, css)

    subprocess.run(["sphinx-build", "-b", "html", outdir, builddir], check=True)

    out_html = os.path.join(builddir, stem + ".html")
    subprocess.run(["open", out_html], check=True)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
