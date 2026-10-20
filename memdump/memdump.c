/*
 * $Id$
 *
 * Copyright (c) 1997-2008, Juniper Networks, Inc.
 * All rights reserved.  See ../Copyright for additional information.
 */

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <ctype.h>
#include <unistd.h>
#include <err.h>
#include <errno.h>
#include <fcntl.h>
#include <sys/stat.h>

#include "memdump.h"

/**
 * Inspect the fisrt byte of a UTF-8 character representation and
 * return the number of bytes in the representation, as indicated by
 * the high-order bits.
 */
static inline int
xo_utf8_len (char ch)
{
    if ((ch & 0x80) == 0x0)   /* 0xxx xxxx */
        return 1;
    if ((ch & 0xe0) == 0xc0)  /* 110x xxxx */
        return 2;
    if ((ch & 0xf0) == 0xe0)  /* 1110 xxxx */
        return 3;
    if ((ch & 0xf8) == 0xf0)  /* 1111 0xxx */
        return 4;
    return -1;			/* Invalid non-utf8 byte */
}

static inline int
xo_utf8_data_bits (int n)
{
    const uint64_t data_bits
	= (0x07 << 4)
	| (0x0f << 3)
	| (0x1f << 2)
	| (0x7f << 1);

    return (data_bits >> n) & 0xff;
}

typedef unsigned int xo_utf8_wchar_t;

xo_utf8_wchar_t
xo_utf8_codepoint (const char *buf, size_t bufsiz, int len)
{
    char b1 = *buf, b2, b3, b4;
    xo_utf8_wchar_t wc = 0;

    /*
     * The caller should really ensure this, but it seems odd to not
     * take a 'bufsiz' parameter, and if we take it, we should test it.
     */
    if (len <= 0 || (size_t) len > bufsiz)
	return -1;

    /* An attempt to unroll the loop */

    /*
     * For determining the "shortest form", consider the following table::
     *
     *  Scalar Value               | Byte 1   | Byte 2   | Byte 3   | Byte 4
     *  00000000 00000000 0aaaaaaa | 0aaaaaaa |          |          |
     *  00000000 00000bbb baaaaaaa | 110bbbba | 10aaaaaa |          |
     *  00000000 ccccbbbb baaaaaaa | 1110cccc | 10cbbbba | 10aaaaaa |
     *  000ddddd dccccbbb baaaaaaa | 11110ddd | 10dddccc | 10cbbbba | 10aaaaaa
     *

     * For each length, 'zeroes' is set to the bits that should be zero.
     */

    /*
     * 'test' is the bits we extract
     * 'match' is the (const) value it must match
     * 'zeros' is the bits that must _not_ be zeros; if they are zero, then
     * the value is encoded in a "non-shortest form", which we reject as
     * directed by Unicode TR-36.  Perhaps we should return another byte?
     */
    xo_utf8_wchar_t test, match, zeros;

    b1 &= xo_utf8_data_bits(len);
    if (len == 1) {
	wc = (unsigned char) b1; /* Make the simple case simple */

    } else {
	if (len == 2) {
	    b2 = buf[1];
	    test = b2 & 0xc0;
	    match = 0x80;
	    wc = (b1 << 6) | (b2 & 0x3f);
	    zeros = b1 & 0b00011110;

	} else if (len == 3) {
	    b2 = buf[1];
	    b3 = buf[2];
	    test = (b2 & 0xc0) << 8 | (b3 & 0xc0);
	    match = 0x8080;
	    wc = b1 << 12 | (b2 & 0x3f) << 6 | (b3 & 0x3f);
	    zeros = (b1 & 0b00001111) << 8 | (b2 & 0b00100000);

	} else if (len == 4) {
	    b2 = buf[1];
	    b3 = buf[2];
	    b4 = buf[3];
	    test = (b2 & 0xc0) << 16 | (b3 & 0xc0) << 8 | (b4 & 0xc0);
	    match = 0x808080;
	    wc = b1 << 18 | (b2 & 0x3f) << 12 | (b3 & 0x3f) << 6 | (b4 & 0x3f);
	    zeros = (b1 & 0b00000111) << 8 | (b2 & 0b00111000);

	} else return -1;

	if (test != match)
	    return -1;
	if (zeros == 0)
	    return -1;
    }

    return wc;
}

int opt_utf8;

static inline int
memdump_utf8_width (int ulen)
{
    if (ulen <= 3)
	return ulen * 2;
    return 6;
}

/*
 * memdump(): dump memory contents in hex/ascii
0         1         2         3         4         5         6         7
0123456789012345678901234567890123456789012345678901234567890123456789012345
XX XX XX XX  XX XX XX XX - XX XX XX XX  XX XX XX XX abcdefghijklmnop
 */
void
memdump (FILE *fp, const char *title, const char *data,
         size_t len, const char *tag, int indent)
{
    enum { MAX_PER_LINE = 16 };
    char buf[80];
    char text[80];
    char utf8[800];
    xo_utf8_wchar_t wc;
    char *bp, *tp, *up;
    size_t i;
    char *ebp = buf + sizeof(buf);
    char *eup = utf8 + sizeof(utf8);

#if 0
    static const int ends[ MAX_PER_LINE ] = { 2, 5, 8, 11, 15, 18, 21, 24,
                                              29, 32, 35, 38, 42, 45, 48, 51 };
#endif

    if (fp == NULL)
	fp = stdout;
    if (tag == NULL)
	tag = "";

    fprintf(fp, "%*s[%s] @ %p (%#lx/%lu)\n", indent + 1, tag,
            title, data, (unsigned long) len, (unsigned long) len);


    while (len > 0) {
        bp = buf;
        tp = text;
	up = utf8;

        for (i = 0; i < MAX_PER_LINE && i < len; i++) {
	    char ch = *data;

            if (i && (i % 4) == 0) *bp++ = ' ';
            if (i == 8) {
                *bp++ = '-';
                *bp++ = ' ';
            }

            snprintf(bp, ebp - bp, "%02x ", (unsigned char) ch);
            bp += strlen(bp);
            *tp++ = (isprint((int) ch) && ch >= ' ') ? ch : '.';

	    if (opt_utf8) {
		if ((ch & 0xc0) == 0xc0) { /* First character */
		    int ulen = xo_utf8_len(ch);
		    wc = xo_utf8_codepoint(data, len - i, ulen);
		    if (wc > 0) {
			snprintf(up, eup - up, " [%zu:%d %0*x]",
				 i, ulen, memdump_utf8_width(ulen), wc);
		    } else {
			snprintf(up, eup - up, " [%zu: err]", i);
		    }
		    up += strlen(up);
		}
	    }

            data += 1;
        }

        *tp = 0;
        *bp = 0;
	*up = 0;
        fprintf(fp, "%*s%-54s%-16s%s\n", indent + 1, tag, buf, text, utf8);
        len -= i;
    }
}

#ifdef MAIN
static char *fake_argv[] = { "-", NULL };

int
main (int argc, char **argv)
{
    for (argv++; *argv; argv++) {
	const char *name = *argv;
	if (name[0] == '-' && name[1] != '\0') {
	    switch (name[1]) {
	    case 'u':
		opt_utf8 = 1;
		break;
	    default:
		errx(1, "unknown option: %c", name[1]);
	    }

	    continue;
	}

	break;
    }

    if (*argv == NULL) {
        argv = fake_argv;
    }

    for ( ; *argv; argv++) {
	const char *name = *argv;
	if (name[0] == '-' && name[1] == '\0') {
	    size_t len = BUFSIZ;
	    char *cp = malloc(len);
	    if (cp == NULL)
		err(1, "out of memory");
	    char *ep = cp + len;
	    char *np = cp;

	    for (;;) {
		int rc = read(0, np, ep - np);
		if (rc == 0) {
		    break;

		} else if (rc < 1) {
		    if (errno == EINTR)
			continue;
		    break;
		}

		np += rc;
		if (np == ep) {
		    char *rp = realloc(cp, 2 * len);
		    if (rp == NULL)
			err(1, "out of memory");

		    len *= 2;
		    ep = rp + len;
		    np = &rp[np - cp];
		    cp = rp;
		}
	    }

	    len = np - cp;
	    memdump(NULL, name, cp, len, NULL, 0);
	    free(cp);

	} else {

	    int fd = open(*argv, O_RDONLY);
	    if (fd < 0)
		err(1, "could not open input file '%s'", *argv);

	    struct stat st;

	    if (fstat(fd, &st) < 0)
		err(1, "could not stat input file '%s'", *argv);

	    char *cp = malloc(st.st_size);
	    if (cp == NULL)
		err(1, "out of memory");

	    int rc = read(fd, cp, st.st_size);
	    if (rc < 0)
		err(1, "read failure");

	    memdump(NULL, name, cp, st.st_size, NULL, 0);
	    free(cp);
	    close(fd);
	}
    }

    return 0;
}
#endif /* MAIN */
