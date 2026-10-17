#
# Tests for claude-sandbox.pl
#
#   make test             # run all tests, diff out/ against saved/
#   make test TESTS=env   # run just one (or a few)
#   make accept           # copy out/ to saved/ (after inspecting it!)
#   make clean            # remove out/
#
# Each test is NAME.sh (sourced by driver.sh), usually with a NAME.conf.
# Output lands in out/NAME.out and out/NAME.err; known-good copies
# live in saved/.
#

TESTS = \
    basic \
    check \
    deny \
    env \
    errors \
    help \
    noconf \
    oneline \
    optional \
    options \
    quoting \
    symlink

all: test

test:
	@mkdir -p out saved
	@status=ok; \
	for t in ${TESTS}; do \
	    sh ./driver.sh $$t || exit 1; \
	    for ext in out err; do \
		f=$$t.$$ext; \
		if [ ! -f saved/$$f ]; then \
		    echo "-- $$f: no saved data"; \
		    [ $$status = fail ] || status=new; \
		elif ! diff -u saved/$$f out/$$f; then \
		    echo "-- $$f: FAILED"; \
		    status=fail; \
		fi; \
	    done; \
	done; \
	case $$status in \
	ok) echo "-- all tests match saved data" ;; \
	new) echo "-- some tests have no saved data" ;; \
	*) echo "-- some tests FAILED"; exit 1 ;; \
	esac

accept:
	@mkdir -p saved
	@for t in ${TESTS}; do \
	    cp out/$$t.out out/$$t.err saved/; \
	done

clean:
	rm -rf out

.PHONY: all test accept clean
