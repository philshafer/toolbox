#!/usr/bin/env python3
#
# Converts the TAIL of a Claude Code session's .jsonl transcript into a
# readable rST Input:/Output: document, the same way bin/bonnie-log.py
# does for a `script`(1) terminal capture -- but reading structured
# message/tool_use/tool_result records instead of replaying a terminal,
# which sidesteps terminal-rendering artifacts (e.g. tabs flattened to a
# fixed width by whatever displayed a diff live) since tool inputs like
# Edit's old_string/new_string carry the real file content untouched.
#
# These transcripts are append-only logs of a session that can be
# resumed over days or weeks, so byte offset has no reliable relationship
# to wall-clock time; "take the tail" is just "take the most recent
# records" by line order, not a time window. Only the last `--fraction`
# of lines (default 15%) is read at all -- `wc -l`/`head`/`tail` do the
# slicing in the shell so the full file (which can be 50-100MB+) is never
# read or parsed in this process. That line-count slice is just a cheap
# starting point, though: a resumed session's tail can still span far
# more wall-clock time than anyone cares to re-read, so records older
# than `--hours` (default 48h, by each record's own "timestamp" field)
# are then dropped from that slice before rendering.
#
# Usage: bonnie-log.py TRANSCRIPT.jsonl [FRACTION] [HOURS]
#        bonnie-log.py --list [COUNT]
#        bonnie-log.py --help
#   FRACTION defaults to 0.15 (last 15% of lines).
#   HOURS defaults to 48 (drop records older than this); 0 disables it.
#   A relative TRANSCRIPT that doesn't exist is looked up under
#   ~/.claude/projects/-, so a name printed by --list works as is.
#   --list shows the COUNT (default 10) most recent transcripts.
#
import calendar
import difflib
import glob
import json
import os
import re
import subprocess
import sys
import time

DEFAULT_FRACTION = 0.15
DEFAULT_HOURS = 48
HEADER_LINES = 1

# Same "always black text, lightened background" rule as bonnie-log.py:
# pick colors ourselves here (there's no captured terminal palette to
# inherit), but keep them light enough that black text stays readable.
DIFF_ADD_BG = "b7f5c0"
DIFF_DEL_BG = "ffc2c2"
INPUT_USER_BG = "f2e2c0"

# Placeholder for a line break inside a hover tooltip's `title` attribute:
# a literal newline (or Unicode line separator) in the :abbr: role's rST
# source collapses to a single space once docutils joins the title text,
# but this ASCII control character survives untouched -- main() swaps it
# for a real newline in the built HTML, after docutils is done with it.
TIP_BREAK = "\x01"


def read_tail_slice(path, fraction):
    """Returns (header_lines, tail_lines, total, n) -- kept separate (rather
    than one combined list) so the header record and the actual slice size
    can be reported in the rendered document."""
    total = int(subprocess.check_output(["wc", "-l", path]).split()[0])
    n = max(1, int(total * fraction))
    tail = subprocess.check_output(
        ["tail", "-n", str(n), path]).decode("utf-8", "replace").splitlines()

    header = []
    start_of_tail = total - n + 1  # 1-indexed first line number in `tail`
    if HEADER_LINES < start_of_tail:
        header = subprocess.check_output(
            ["head", "-n", str(HEADER_LINES), path]
        ).decode("utf-8", "replace").splitlines()
    return header, tail, total, n


def parse_timestamp(ts):
    """Parses a record's ISO-8601 "...Z" timestamp to epoch seconds
    (UTC); returns None if it's missing or doesn't parse, rather than
    guessing -- callers should treat an unparseable timestamp as "keep
    it", not as "it's old"."""
    if not isinstance(ts, str):
        return None
    try:
        dt = ts.rstrip("Z").split(".")[0]
        return calendar.timegm(time.strptime(dt, "%Y-%m-%dT%H:%M:%S"))
    except ValueError:
        return None


def filter_by_age(lines, hours):
    """Drops lines whose record's own "timestamp" predates `hours` ago.
    The line-count slice from read_tail_slice() only bounds how much is
    read off disk -- a session resumed over days/weeks can still have a
    tail spanning far more than the window actually wanted, so this
    narrows it further by wall-clock time. A line with no parseable
    timestamp is kept rather than guessed at."""
    if not hours:
        return lines
    cutoff = time.time() - hours * 3600
    kept = []
    for line in lines:
        stripped = line.strip()
        if not stripped:
            kept.append(line)
            continue
        try:
            obj = json.loads(stripped)
        except ValueError:
            kept.append(line)
            continue
        epoch = parse_timestamp(obj.get("timestamp")) if isinstance(obj, dict) else None
        if epoch is not None and epoch < cutoff:
            continue
        kept.append(line)
    return kept


def parse_records(lines):
    records = []
    for line in lines:
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            continue
        if obj.get("type") in ("user", "assistant"):
            records.append(obj)
    return records


def message_content(record):
    content = record.get("message", {}).get("content")
    if isinstance(content, str):
        return [{"type": "text", "text": content}]
    if isinstance(content, list):
        return content
    return []


def is_human_input(record):
    """A `user`-type record is either something the person actually
    typed, or the harness feeding a tool_result back to the assistant --
    only the former belongs in an "Input:" section; the latter is
    rendered inline with the tool_use it answers."""
    if record.get("type") != "user":
        return False
    blocks = message_content(record)
    if any(b.get("type") == "tool_result" for b in blocks if isinstance(b, dict)):
        return False
    return True


def human_text(record):
    parts = []
    for b in message_content(record):
        if isinstance(b, dict) and b.get("type") == "text":
            parts.append(b.get("text", ""))
        elif isinstance(b, str):
            parts.append(b)
    return "\n".join(p for p in parts if p).strip()


def index_tool_results(records):
    """Maps tool_use_id -> (tool_result content block, toolUseResult dict)."""
    results = {}
    for rec in records:
        if rec.get("type") != "user":
            continue
        tur = rec.get("toolUseResult")
        for b in message_content(rec):
            if isinstance(b, dict) and b.get("type") == "tool_result":
                results[b.get("tool_use_id")] = (b, tur if isinstance(tur, dict) else None)
    return results


def diff_lines(old, new):
    """Line-level diff computed from the tool's own old/new strings --
    these still have real tab characters, unlike a pre-rendered diff
    view, so expanding tabs here (from column 0 of each line) lines
    code up the way the file itself is actually indented."""
    out = []
    for line in difflib.ndiff(old.splitlines(), new.splitlines()):
        tag, text = line[0], line[2:]
        if tag == "?":
            continue
        out.append((tag, text.expandtabs(8)))
    return out


def escape_markup(text):
    # Assistant/user text is markdown and C, not rST: backtick, asterisk
    # and pipe are rST markup delimiters (interpreted text, emphasis,
    # substitution reference) that routinely appear in prose/code as a
    # code-span, a pointer deref, or bitwise-or, and a trailing
    # underscore (e.g. a C macro prefix like "XFSF_*") is *also* markup
    # on its own, with no backtick needed -- it's docutils' named
    # hyperlink reference syntax, "target_" -- so each needs escaping or
    # docutils either misparses them as markup or fails outright with
    # "Unknown target name" / "Undefined substitution referenced".
    return (text.replace("\\", r"\\").replace("`", r"\`")
            .replace("*", r"\*").replace("|", r"\|").replace("_", r"\_"))


# Human/assistant prose is markdown, not rST -- convert the handful of
# constructs that actually show up in chat text (code spans, bold,
# italic, links) to their rST equivalents instead of just escaping them
# flat, so they render as formatted text rather than literal asterisks
# and backticks. Anything that's not recognized markdown falls through
# escape_markup() unchanged, same as before.
_INLINE_RE = re.compile(
    r'`(?P<code>[^`\n]+)`'
    r'|\*\*(?P<bold>[^*\n]+?)\*\*'
    r'|\*(?P<italic>[^*\n]+?)\*'
    r'|(?<!\w)_(?P<italic_u>[^_\n]+?)_(?!\w)'
    r'|\[(?P<link_text>[^\]\n]+)\]\((?P<link_url>[^)\n]+)\)'
)


def inline_to_rst(text):
    # rST requires an inline-markup start-string to be preceded by
    # whitespace/start-of-text (never a bare word/symbol char) and an
    # end-string to be followed likewise -- markdown has no such rule, so
    # chat text routinely violates it (e.g. "`not`->1" or "`#define`d").
    # A zero-width "\ " escape on the offending side satisfies docutils
    # without changing how the text looks once rendered.
    out = []
    pos = 0
    for m in _INLINE_RE.finditer(text):
        out.append(escape_markup(text[pos:m.start()]))
        prev_char = text[m.start() - 1] if m.start() > 0 else ""
        next_char = text[m.end()] if m.end() < len(text) else ""
        lead = r"\ " if prev_char and not prev_char.isspace() else ""
        trail = r"\ " if next_char and not next_char.isspace() else ""
        if m.group("code") is not None:
            markup = "``%s``" % m.group("code")
        elif m.group("bold") is not None:
            markup = "**%s**" % escape_markup(m.group("bold"))
        elif m.group("italic") is not None:
            markup = "*%s*" % escape_markup(m.group("italic"))
        elif m.group("italic_u") is not None:
            markup = "*%s*" % escape_markup(m.group("italic_u"))
        else:
            markup = "`%s <%s>`_" % (escape_markup(m.group("link_text")),
                                      m.group("link_url"))
        out.append(lead + markup + trail)
        pos = m.end()
    out.append(escape_markup(text[pos:]))
    return "".join(out)


_FENCE_RE = re.compile(r'^(```|~~~)\s*([\w+-]*)\s*$')
_HEADER_RE = re.compile(r'^(#{1,6})\s+(.*)$')
_LIST_RE = re.compile(r'^(\s*)([-*+]|\d+[.)])\s+(.*)$')
_QUOTE_RE = re.compile(r'^>\s?(.*)$')


def markdown_to_rst(text):
    lines = text.splitlines()
    out = []
    i = 0
    n = len(lines)
    in_list = False
    last_list_indent = None

    def blank_before():
        if out and out[-1] != "":
            out.append("")

    while i < n:
        line = lines[i]
        stripped = line.strip()

        m = _FENCE_RE.match(stripped)
        if m:
            fence, lang = m.group(1), m.group(2) or "text"
            i += 1
            code_lines = []
            while i < n and lines[i].strip() != fence:
                code_lines.append(lines[i])
                i += 1
            if i < n:
                i += 1
            blank_before()
            # Nested inside a list item -- indent the directive (and its
            # content) to the item's continuation column. docutils' rule
            # for where a directive's indented content block ends is "the
            # next line flush with the left margin", not "less indented
            # than the first content line" -- so a column-0 directive
            # sitting under a list silently swallows the next list item's
            # text (itself indented, just less) as more of its own body.
            pad = " " * (last_list_indent + 2) if in_list else ""
            out.append(pad + ".. code-block:: %s" % lang)
            out.append("")
            for cl in code_lines:
                out.append(pad + "   " + cl if cl else "")
            if not code_lines:
                out.append("")
            out.append("")
            continue

        m = _HEADER_RE.match(stripped)
        if m:
            blank_before()
            out.append(".. rubric:: " + inline_to_rst(m.group(2).strip()))
            out.append("")
            i += 1
            in_list = False
            continue

        m = _LIST_RE.match(line)
        if m:
            indent, marker, rest = m.groups()
            if not in_list:
                blank_before()
                in_list = True
            elif len(indent) != last_list_indent:
                # rST (unlike markdown) requires a blank line before a
                # nested list -- a bare indent change is "unexpected
                # indentation" to docutils, not an implicit sub-list.
                blank_before()
            last_list_indent = len(indent)
            bullet = "-" if marker in ("-", "*", "+") else marker
            out.append("%s%s %s" % (indent, bullet, inline_to_rst(rest)))
            i += 1
            continue

        if in_list:
            if stripped == "":
                in_list = False
                out.append("")
                i += 1
                continue
            line_indent = len(line) - len(line.lstrip(" \t"))
            if line[:1] in (" ", "\t") and line_indent > last_list_indent:
                # Indented further than the bullet's own marker column --
                # a genuine continuation of that item's paragraph. Merely
                # non-zero indentation isn't enough: rST requires at
                # least the bullet's *content* column (marker + 1), not
                # just its marker column, or it's an under-indented
                # dedent docutils can't attach to the item.
                out.append(inline_to_rst(line))
                i += 1
                continue
            in_list = False
            blank_before()

        m = _QUOTE_RE.match(stripped)
        if m:
            blank_before()
            out.append("    " + inline_to_rst(m.group(1)))
            i += 1
            continue

        if stripped == "":
            out.append("")
            i += 1
            continue

        out.append(inline_to_rst(line))
        i += 1

    return "\n".join(out)


def format_header_field(key):
    spaced = re.sub(r'(?<!^)(?=[A-Z])', ' ', key)
    return spaced[:1].upper() + spaced[1:]


def header_info_fields(header_lines):
    """Pulls whatever scalar fields are present on the transcript's header
    record (the first line of the .jsonl -- its exact shape isn't fixed
    across Claude Code versions) for the informational block at the top
    of the rendered document."""
    if not header_lines:
        return []
    try:
        obj = json.loads(header_lines[0])
    except ValueError:
        return []
    if not isinstance(obj, dict):
        return []
    fields = []
    for key, value in obj.items():
        if isinstance(value, (dict, list)):
            continue
        value = str(value)
        if len(value) > 120:
            value = value[:117] + "..."
        fields.append((format_header_field(key), value))
    return fields


def render_diff_line(tag, text):
    marker = {"+": "+", "-": "-", " ": " "}[tag]
    full = marker + " " + text
    if tag == " ":
        return escape_markup(full)
    role = "diff-add" if tag == "+" else "diff-del"
    lead_len = len(full) - len(full.lstrip())
    trail_len = len(full) - len(full.rstrip())
    lead = full[:lead_len]
    trail = full[len(full) - trail_len:] if trail_len else ""
    core = full[lead_len:len(full) - trail_len] if trail_len else full[lead_len:]
    if not core:
        return full
    return lead + (":%s:`%s`" % (role, escape_markup(core))) + trail


MAX_PLAIN_LINES = 40
MAX_PLAIN_CHARS = 4000


def plain_text_lines(text):
    text = text.expandtabs(8)
    lines = text.splitlines()
    truncated = False
    if len(lines) > MAX_PLAIN_LINES:
        lines = lines[:MAX_PLAIN_LINES]
        truncated = True
    joined = "\n".join(lines)
    if len(joined) > MAX_PLAIN_CHARS:
        joined = joined[:MAX_PLAIN_CHARS]
        truncated = True
        lines = joined.splitlines()
    if truncated:
        lines.append("... (truncated)")
    return [escape_markup(l) for l in lines]


def describe_tool_use(name, input_):
    for key in ("file_path", "path", "command", "url", "pattern", "query"):
        if key in input_:
            val = input_[key]
            if isinstance(val, str):
                val = val.expandtabs(8).replace("\n", " ")
                if len(val) > 80:
                    val = val[:77] + "..."
                return "%s(%s)" % (name, escape_markup(val))
    return "%s(...)" % name


def render_tool_use(block, tool_results, out_lines):
    name = block.get("name", "Tool")
    input_ = block.get("input", {}) or {}
    out_lines.append("   ⏺ %s" % describe_tool_use(name, input_))

    result = tool_results.get(block.get("id"))
    if result is None:
        return
    _result_block, tur = result

    if tur and isinstance(tur.get("oldString"), str) and isinstance(tur.get("newString"), str):
        for tag, text in diff_lines(tur["oldString"], tur["newString"]):
            out_lines.append("      " + render_diff_line(tag, text))
        return

    if isinstance(input_.get("content"), str) and "file_path" in input_:
        out_lines.append("      (new file)")
        for line in plain_text_lines(input_["content"]):
            out_lines.append("      " + line)
        return

    content = _result_block.get("content")
    if isinstance(content, list):
        content = "\n".join(
            c.get("text", "") for c in content if isinstance(c, dict))
    if isinstance(content, str) and content.strip():
        for line in plain_text_lines(content):
            out_lines.append("      " + line)


def render_assistant_blocks(blocks, tool_results, segments):
    """Appends (kind, payload) segments: ("text", markdown_str) for prose,
    rendered later as real rST (bold/code/lists/etc.), and ("tool", lines)
    for tool_use/tool_result output, rendered later as a literal block --
    these can't share one representation since only the former is
    markdown."""
    for b in blocks:
        if not isinstance(b, dict):
            continue
        if b.get("type") == "text":
            text = b.get("text", "").strip()
            if text:
                segments.append(("text", text))
        elif b.get("type") == "tool_use":
            tool_lines = []
            render_tool_use(b, tool_results, tool_lines)
            segments.append(("tool", tool_lines))
        # "thinking" blocks are intentionally omitted from the transcript.


def record_meta(rec):
    """Pulls the record's own type/subtype/timestamp/cwd -- shown as a
    hover tooltip on each Input:/Output: section title so the record(s)
    behind it are identifiable without opening the .jsonl. "subtype"
    only ever shows up on non-user/assistant records (e.g. the harness's
    "away_summary" system records), so it's typically absent here."""
    fields = []
    for key in ("type", "subtype", "timestamp", "cwd"):
        val = rec.get(key)
        if val is not None:
            fields.append((key, str(val)))
    return fields


def segment_turns(records, tool_results):
    preamble = []
    preamble_meta = [None]
    turns = []  # each: [text, input_meta, output_meta, segments]
    cur_turn = None

    for rec in records:
        if is_human_input(rec):
            text = human_text(rec)
            if not text:
                continue
            cur_turn = [text, record_meta(rec), None, []]
            turns.append(cur_turn)
        elif rec.get("type") == "assistant":
            if cur_turn is not None:
                target = cur_turn[3]
                if cur_turn[2] is None:
                    cur_turn[2] = record_meta(rec)
            else:
                target = preamble
                if preamble_meta[0] is None:
                    preamble_meta[0] = record_meta(rec)
            render_assistant_blocks(message_content(rec), tool_results, target)

    return preamble, preamble_meta[0], turns


def parsed_literal(lines):
    if not lines:
        return []
    out = [".. parsed-literal::", ""]
    out.extend("   " + line if line else "" for line in lines)
    return out


def render_segments(segments, out):
    for kind, payload in segments:
        if kind == "text":
            rst_text = markdown_to_rst(payload)
            if rst_text.strip():
                if out and out[-1] != "":
                    out.append("")
                out.extend(rst_text.splitlines())
                out.append("")
        else:  # "tool" -- not markdown, keep as a literal block
            if out and out[-1] != "":
                out.append("")
            out.extend(parsed_literal(payload))
            out.append("")


def section_title(label, meta):
    """`label` alone, or -- when `meta` has anything -- `label` wrapped
    in docutils' built-in :abbr: role so the type/subtype/timestamp/cwd
    fields show up as a native HTML hover tooltip on the title. An
    Input: section whose record type is "user" uses the "input-user"
    role instead of the bare "abbr" role -- same tooltip behavior
    (:abbr: is its base class), but it also carries the "input-user"
    CSS class so that section gets a pale tan background."""
    if not meta:
        return label
    role = "abbr"
    if label == "Input:" and any(k == "type" and v == "user" for k, v in meta):
        role = "input-user"
    tip = TIP_BREAK.join("%s: %s" % (k, v) for k, v in meta).replace("`", "'")
    return ":%s:`%s (%s)`" % (role, label, tip)


def append_section(out, label, meta):
    title = section_title(label, meta)
    out.append(title)
    out.append("-" * len(title))
    out.append("")


def render_transcript(preamble, preamble_meta, turns):
    out = []
    if preamble:
        append_section(out, "Output:", preamble_meta)
        render_segments(preamble, out)

    for input_text, input_meta, output_meta, output_segments in turns:
        append_section(out, "Input:", input_meta)
        out.extend(markdown_to_rst(input_text).splitlines())
        out.append("")

        if output_segments:
            append_section(out, "Output:", output_meta)
            render_segments(output_segments, out)

    return "\n".join(out) + "\n"


DIFF_CSS = """\
.diff-add {
    color: black;
    background-color: #%s;
}
.diff-del {
    color: black;
    background-color: #%s;
}
.input-user {
    background-color: #%s;
}
""" % (DIFF_ADD_BG, DIFF_DEL_BG, INPUT_USER_BG)


def convert(path, fraction, hours, stem):
    header_lines, tail_lines, total, n = read_tail_slice(path, fraction)
    tail_lines = filter_by_age(tail_lines, hours)
    kept = len(tail_lines)
    records = parse_records(header_lines + tail_lines)
    tool_results = index_tool_results(records)
    preamble, preamble_meta, turns = segment_turns(records, tool_results)
    transcript = render_transcript(preamble, preamble_meta, turns)

    title = stem.replace("_", " ").replace("-", " ")
    header = [title, "=" * len(title), ""]

    mtime_str = time.strftime(
        "%Y-%m-%d %H:%M:%S %Z", time.localtime(os.path.getmtime(path)))
    header.append(":Source file: %s" % escape_markup(path))
    header.append(":Source file timestamp: %s" % mtime_str)
    for label, value in header_info_fields(header_lines):
        header.append(":%s: %s" % (label, escape_markup(value)))
    header.append(":Slice rendered: last %g%% of lines (%d of %d)" %
                   (fraction * 100, n, total))
    if hours:
        header.append(":Age filter: entries older than %gh dropped (kept %d of %d read)" %
                       (hours, kept, n))
    header.append("")
    header.append(".. role:: diff-add")
    header.append(".. role:: diff-del")
    header.append(".. role:: input-user(abbr)")
    header.append("")

    rst = "\n".join(header) + "\n" + transcript
    return rst, DIFF_CSS


CONF_PY = """\
project = %(project)r
master_doc = %(stem)r
extensions = []
html_css_files = [%(css)r]
html_static_path = ["_static"]
exclude_patterns = ["_build"]
"""


def write_sphinx_project(outdir, stem, rst, css):
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


USAGE = """\
usage: bonnie-log.py TRANSCRIPT.jsonl [FRACTION] [HOURS]
       bonnie-log.py --list [COUNT]
       bonnie-log.py --help

  TRANSCRIPT  a Claude Code session transcript; a relative name that
              doesn't exist is looked up under ~/.claude/projects/-
              (so the names --list prints work as is)
  FRACTION    read only the last FRACTION of lines (default %g)
  HOURS       drop records older than HOURS (default %g; 0 keeps all)
  --list      show the COUNT (default 10) most recently changed
              transcripts, newest first
""" % (DEFAULT_FRACTION, DEFAULT_HOURS)

DEFAULT_LIST_COUNT = 10


def projects_prefix():
    """Where Claude Code keeps transcripts: one directory per project,
    named for its path with "/" turned into "-", so every directory
    name starts with "-"."""
    return os.path.join(os.path.expanduser("~"), ".claude", "projects", "-")


def list_transcripts(count):
    prefix = projects_prefix()
    paths = glob.glob(prefix + "*/*.jsonl")
    paths.sort(key=os.path.getmtime, reverse=True)
    for path in paths[:count]:
        print(path[len(prefix):])
    return 0


def find_transcript(path):
    """`path` itself if it exists (or is absolute); otherwise the same
    name under the projects directory, as --list prints it."""
    if os.path.exists(path) or os.path.isabs(path):
        return path
    alt = projects_prefix() + path
    return alt if os.path.exists(alt) else path


def main(argv):
    import getpass

    if len(argv) < 2:
        sys.stderr.write(USAGE)
        return 1

    if argv[1] in ("-h", "--help"):
        sys.stdout.write(USAGE)
        return 0

    if argv[1] == "--list":
        try:
            count = int(argv[2]) if len(argv) > 2 else DEFAULT_LIST_COUNT
        except ValueError:
            sys.stderr.write("bonnie-log.py: bad --list count: %s\n" % argv[2])
            return 1
        return list_transcripts(count)

    if argv[1].startswith("-"):
        sys.stderr.write("bonnie-log.py: unknown option: %s\n" % argv[1])
        sys.stderr.write(USAGE)
        return 1

    logfile = find_transcript(argv[1])
    if not os.path.exists(logfile):
        sys.stderr.write("bonnie-log.py: %s: no such file\n" % argv[1])
        return 1
    fraction = float(argv[2]) if len(argv) > 2 else DEFAULT_FRACTION
    hours = float(argv[3]) if len(argv) > 3 else DEFAULT_HOURS
    stem = "transcript"

    user = os.environ.get("USER") or getpass.getuser()
    outdir = "/tmp/%s-bonnie" % user
    os.makedirs(outdir, exist_ok=True)
    builddir = os.path.join(outdir, "_build", "html")

    rst, css = convert(logfile, fraction, hours, stem)
    write_sphinx_project(outdir, stem, rst, css)

    subprocess.run(["sphinx-build", "-b", "html", outdir, builddir], check=True)

    # docutils collapses a real newline inside a :abbr: role's source text
    # to a single space, so TIP_BREAK stood in for the line break through
    # the build; swap it for a real newline now that docutils is done --
    # browsers render an embedded newline in a `title` attribute as a
    # multi-line tooltip, but only if it's a real newline byte.
    out_html = os.path.join(builddir, stem + ".html")
    with open(out_html, "r", encoding="utf-8") as f:
        html = f.read()
    html = html.replace(TIP_BREAK, "\n")
    # Sphinx wraps each rST section in its own <section id="...">...
    # </section>; the "input-user" class lives only on the <abbr> inside
    # the heading (that's as close to the title as the rST role markup
    # can reach), which only tints the title text. Carry the class onto
    # the enclosing <section> too so the pale tan background covers the
    # whole Input: block, not just its heading.
    html = re.sub(
        r'<section id="([^"]*)">(\s*<h2><abbr class="input-user")',
        r'<section id="\1" class="input-user">\2',
        html)
    with open(out_html, "w", encoding="utf-8") as f:
        f.write(html)

    subprocess.run(["open", out_html], check=True)

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
