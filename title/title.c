/* Rexx? */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/types.h>
#include <ctype.h>
#include <strings.h>

#define NODOMAINS	/* Never put domains into messages */

char lost[] = "(lost)";
char dir[200], hostname[100], outbuf[BUFSIZ];

int
main (int argc, char **argv)
{
    char *home, *display, *domain, *cp, *path, *title;
    char *wd = dir, *phost = hostname, *host = hostname, *outline = outbuf;
    char *cell, *ssh_conn, *macosx, *term;
    int  i, len, root = 0, local = 0;

    display = getenv("DISPLAY");
    ssh_conn = getenv("SSH_CONNECTION");
    macosx = getenv("MACOSX");
    term = getenv("TERM");

    if (geteuid() == 0) root = 1;

    if (argc >= 2) outline = argv[1];
    else {
	if (gethostname(host, sizeof(hostname)) < 0) host = lost;
	if (islower(*host)) *host = toupper(*host);

	if (!display) {
	    domain = index(host, '.');
	    if (domain) *domain = '\0';
	} else {
	    domain = index(display, '.');
	    cp = index(display, ':');
	    if (display[0] == 'u' && display[1] == 'n' && display[2] == 'i'
		    && display[3] == 'x' && display[4] == ':')
		display = cp;
	    if (cp == display) {
		local = 1;
		phost = "";
		domain = index(host, '.');
		if (domain != 0) *domain = '\0';
	    } else {

#ifdef NODOMAINS
		cp = index(host, '.');
		if (cp != 0) *cp = '\0';
#else
		if (domain != 0 && cp != 0 && domain < cp) {
		    *cp = '\0';
		    cp = index(host, '.');
		    if (cp != 0 && strcmp(domain, cp) == 0)
			*cp = '\0';
		}
#endif
	    }
	}

	if (getcwd(wd, sizeof(dir)) == 0) wd = lost;
	home = getenv("HOME");
	len = home ? strlen(home) : 0;

	if (len > 1 && strncmp(home, wd, len) == 0) {
	    wd += len;
	    if (*wd == '/') wd += 1;
	}

	if (root) cp = "# ";
	else if (local && *wd == '\0') cp = "~";
	else if (local || *wd == '\0') cp = "";
	else cp = ": ";

	cell = getenv("AFSCELLNAME");
	len = cell ? strlen(cell) : 0;

	if (len > 1 && strncmp(cell, wd, len) == 0) {
	    wd += len;
	    if (*wd == '/') wd += 1;
	    cell = "@";
	} else cell = "";

	title = getenv("TITLE");
	if (title == 0) title = "";

	sprintf(outbuf, "%s%s%s%s%s", title, phost, cp, cell, wd);

	cp = rindex(wd, '/');
	if (cp == 0) path = wd;
	else {
	    *cp = '\0';
	    path = rindex(wd, '/');
	    *cp = '/';
	    if (path == 0) path = wd;
	    else {
		if (path != wd) path += 1;
		cell = "";
	    }
	}
	if (*path == 0 && *cell) {
	    path = cell;
	    cell = "";
	}

	fprintf(stdout, "%s%s%s %s%s%s%s%% ", title, root ? "#### " : "", host,
		*path ? "[" : "", cell, path, *path ? "]" : "");
    }

    if (display || ssh_conn || macosx)
	fprintf(stderr, "\033]2;%s\007\033]1;%s\007", outline, outline);

    if (term && strcmp(term, "screen") == 0)
	fprintf(stderr, "\033k%s\033\\", outline);

    exit(0);
}
