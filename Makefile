#
# Install symlinks to toolkit
#

DESTDIR=${HOME}/bin
BASELINK=${DESTDIR}/.toolkit
REPODIR=`pwd`

all: base-link bin-files tools

base-link: ${BASELINK}

${BASELINK}:
	ln -s ${REPODIR} ${BASELINK}

# DEB="set -x;"
DEB=@

# FORCE=.
FORCE=

bin-files:
	${DEB} for file in `cd bin; find . -type f`; do \
	    base=`basename $$file` ; \
	    if [ -L ${DESTDIR}/$$base ]; then \
		old=`readlink ${DESTDIR}/$$base`; \
		if [ "$$old" != .toolkit/bin/$$base ]; then \
		    echo "[ $$base is another link: $$old ]"; \
		fi ; \
	    elif [ -f ${DESTDIR}/$$base -a -z ${FORCE} ]; then \
		echo "[ $$base is a file ]"; \
	    else \
		echo "+ $$base" ; \
		ln -s -f .toolkit/bin/$$base ${DESTDIR}/$$base; \
	    fi ; \
	done

tools:  build-title install-ytplay install-claude-sandbox

build-title: ${DESTDIR}/title

${DESTDIR}/title: src/title/title.c ${FORCE}
	cc -O -o ${DESTDIR}/title src/title/title.c

install-ytplay: ${DESTDIR}/ytplay

${DESTDIR}/ytplay: ytplay/ytplay.sh ${FORCE}
	ln -s -f .toolkit/ytplay/ytplay.sh ${DESTDIR}/ytplay

install-claude-sandbox: ${DESTDIR}/claude-sandbox

CSB = claude-sandbox

${DESTDIR}/${CSB}: ${CSB}/${CSB}.pl ${FORCE}
	ln -s -f .toolkit/${CSB}/${CSB}.pl ${DESTDIR}/${CSB}

