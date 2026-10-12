#!/bin/sh
#
# ytplay: embed a youtube.com video in a bare local HTML page and open
# it in the browser, mimicking the way sites like x.com embed the
# youtube player (no ads, no surrounding cruft).
#
# Modeled on the /tmp file handling in libxo's xohtml.sh.in.
#

DONE=
KEEP=
FILE=
URL=
AUTOPLAY=1
PORT=8934

do_help () {
    echo "ytplay: embed a youtube video in a local HTML page"
    echo "Usage: ytplay [options] <youtube-url>"
    echo "Valid options are:"
    echo "    -f | --file <output-file>   Write output to the given file"
    echo "    -k | --keep                 Keep the output file (no auto-delete)"
    echo "    -n | --no-autoplay          Don't autoplay the video"
    echo "    -p | --port <port>          Port for the local http server (default $PORT)"
    exit 1
}

while [ -z "$DONE" -a -n "$1" ]; do
    case "$1" in
        -f|--file)
            shift;
            FILE="$1";
            shift;
            ;;
        -k|--keep)
            shift;
            KEEP=1;
            ;;
        -n|--no-autoplay)
            shift;
            AUTOPLAY=0;
            ;;
        -p|--port)
            shift;
            PORT="$1";
            shift;
            ;;
        -h|--help)
            do_help
            ;;
        -*)
            do_help
            ;;
        *)
            DONE=1;
            URL="$1";
            shift;
            ;;
    esac
done

if [ -z "$URL" ]; then
    read -p "URL: " -e URL
    if [ -z "$URL" ]; then
        do_help
    fi
fi

# Pull the video id out of the usual youtube URL shapes:
#   https://www.youtube.com/watch?v=ID...
#   https://youtu.be/ID...
#   https://www.youtube.com/embed/ID...
#   https://www.youtube.com/shorts/ID...
VIDEO_ID=`printf '%s' "$URL" | sed -n \
    -e 's/^.*[?&]v=\([A-Za-z0-9_-]\{11\}\).*$/\1/p' \
    -e 's#^.*youtu\.be/\([A-Za-z0-9_-]\{11\}\).*$#\1#p' \
    -e 's#^.*/embed/\([A-Za-z0-9_-]\{11\}\).*$#\1#p' \
    -e 's#^.*/shorts/\([A-Za-z0-9_-]\{11\}\).*$#\1#p' \
    | head -n 1`

if [ -z "$VIDEO_ID" ]; then
    echo "ytplay: could not find a video id in '$URL'" 1>&2
    exit 1
fi

# Default output goes to an auto-named file under /tmp, which we then
# open in a browser.  "-f" opts out of both: the caller gets exactly
# the file they asked for and nothing gets opened for them.
#
# The file has to be fetched over http, not file://: youtube's iframe
# player refuses to initialize with a "Video player configuration
# error" when its embedding page has a null/file origin.  So we serve
# our /tmp dir with a small local http server (started once, reused
# by later runs via the pidfile) instead of opening the file directly.
OPEN=
if [ -z "$FILE" ]; then
    DIR="/tmp/tplay.${USER}"
    mkdir -p "$DIR"

    # Sweep old plain runs.  Files saved with -k/--keep use a
    # different prefix (ytplay-keep.*) and are never touched here.
    rm -f "$DIR"/ytplay.*.html

    TS=`/bin/date "+%y%m%d%H%M.%S"`
    if [ -n "$KEEP" ]; then
        FILE="$DIR/ytplay-keep.${TS}$$.html"
    else
        FILE="$DIR/ytplay.${TS}$$.html"
    fi
    OPEN=1
fi

exec > "$FILE"

if [ "$AUTOPLAY" = 1 ]; then
    EMBED_SRC="https://www.youtube.com/embed/${VIDEO_ID}?autoplay=1"
else
    EMBED_SRC="https://www.youtube.com/embed/${VIDEO_ID}"
fi

cat <<EOF
<html>
<head>
<meta http-equiv="content-type" content="text/html; charset=utf-8"/>
<title>ytplay: ${VIDEO_ID}</title>
<style>
  html, body { margin: 0; padding: 0; background: #000; height: 100%; }
  .ytplay-wrap {
    position: fixed;
    top: 0; left: 0; right: 0; bottom: 0;
  }
  .ytplay-wrap iframe {
    width: 100%;
    height: 100%;
    border: 0;
  }
</style>
</head>
<body>
<div class="ytplay-wrap">
<iframe src="${EMBED_SRC}"
    title="YouTube video player"
    frameborder="0"
    allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture; web-share"
    allowfullscreen></iframe>
</div>
</body>
</html>
EOF

exec 1>&2

# Start (or reuse) a local http server rooted at $DIR so the file can
# be fetched as http://localhost:$PORT/... instead of file://.
PIDFILE="$DIR/.ytplay-httpd.pid"
SERVING=

if [ -n "$OPEN" ]; then
    if [ -f "$PIDFILE" ] && kill -0 "`cat "$PIDFILE"`" 2>/dev/null; then
        SERVING=1
    elif command -v python3 >/dev/null 2>&1; then
        ( cd "$DIR" && exec python3 -m http.server "$PORT" --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > "$PIDFILE" )
        i=0
        while [ $i -lt 20 ]; do
            if command -v curl >/dev/null 2>&1 && curl -s -o /dev/null "http://localhost:${PORT}/"; then
                SERVING=1
                break
            fi
            sleep 0.2
            i=`expr $i + 1`
        done
    else
        echo "ytplay: python3 not found, can't start a local http server" 1>&2
        echo "ytplay: falling back to file:// (youtube may show a player error)" 1>&2
    fi
fi

if [ -n "$OPEN" ]; then
    if [ -n "$SERVING" ]; then
        URL_OUT="http://localhost:${PORT}/`basename "$FILE"`"
    else
        URL_OUT="$FILE"
    fi
    if command -v open >/dev/null 2>&1; then
        open "$URL_OUT"
    elif command -v xdg-open >/dev/null 2>&1; then
        xdg-open "$URL_OUT"
    else
        echo "ytplay: output written to $FILE"
    fi
fi

exit 0
