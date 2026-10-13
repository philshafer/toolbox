#
# Install symlinks to toolbox
#

DESTDIR=${HOME}/bin
BASELINK=${DESTDIR}/.toolbox
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
	    if [ -L ${DESTDIR}/$$base -a -z "${FORCE}" ]; then \
		old=`readlink ${DESTDIR}/$$base`; \
		if [ "$$old" != .toolbox/bin/$$base ]; then \
		    echo "[ $$base is another link: $$old ]"; \
		fi ; \
	    elif [ -f ${DESTDIR}/$$base -a -z "${FORCE}" ]; then \
		echo "[ $$base is a file ]"; \
	    else \
		echo "+ $$base" ; \
		ln -s -f .toolbox/bin/$$base ${DESTDIR}/$$base; \
	    fi ; \
	done

INSTALL_TOOLS = \
    install-claude-sandbox \
    install-memdump \
    install-title \
    install-ytplay

tools:  ${INSTALL_TOOLS}

install-title: ${DESTDIR}/title

${DESTDIR}/title: title/title.c ${FORCE}
	cc -O -o ${DESTDIR}/title title/title.c

install-ytplay: ${DESTDIR}/ytplay

${DESTDIR}/ytplay: ytplay/ytplay.sh ${FORCE}
	ln -s -f .toolbox/ytplay/ytplay.sh ${DESTDIR}/ytplay

install-claude-sandbox: ${DESTDIR}/claude-sandbox

CSB = claude-sandbox

${DESTDIR}/${CSB}: ${CSB}/${CSB}.pl ${FORCE}
	ln -s -f .toolbox/${CSB}/${CSB}.pl ${DESTDIR}/${CSB}

install-memdump: ${DESTDIR}/memdump

${DESTDIR}/memdump: memdump/memdump.c ${FORCE}
	cc -O -DMAIN -o ${DESTDIR}/memdump memdump/memdump.c

