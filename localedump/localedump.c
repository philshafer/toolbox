#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <locale.h>

static void
dump (const char *name, const char *value)
{
    if (value && *value)
	printf("%-20s: [%s]\n", name, value);
}

static void
dumpc (const char *name, const char value)
{
    if (value != 0)
	printf("%-20s: [%d]\n", name, value);
}

static void
dumpsx (const char *name, const char *value)
{
    char buf[BUFSIZ], *cp, *ep;
    const unsigned char *vp = (const unsigned char *) value;

    for (cp = buf, ep = buf + sizeof(buf); *vp; vp++) {
	snprintf(cp, ep - cp, "%02x ", *vp);
	cp += 3;
    }

    if (cp > buf)
	cp -= 1;
    *cp = '\0';

    printf("%-20s: [%s] [%s]\n", name, value, buf);
}

static void
dumpx (const char *name, const char *value)
{
    char buf[BUFSIZ], *cp, *ep;
    const unsigned char *vp = (const unsigned char *) value;

    for (cp = buf, ep = buf + sizeof(buf); *vp; vp++) {
	snprintf(cp, ep - cp, "%02x ", *vp);
	cp += 3;
    }

    if (cp > buf)
	cp -= 1;
    *cp = '\0';

    printf("%-20s: [%s]\n", name, buf);
}

static void
localedump (const char *loc)
{
    float f = 123456789.1234;
    if (setlocale(LC_ALL, loc) == NULL) {
	printf ("setlocale failed for '%s'\n", loc);
	exit(1);
    }

    printf("%-20s: [%s]\n", "locale", loc);
    printf("%-20s: [%'.2f]\n", "example", f);

    struct lconv *conv = localeconv();
    dump("decimal_point", conv->decimal_point);
    dump("thousands_sep", conv->thousands_sep);
    dumpx("grouping", conv->grouping);
    dump("int_curr_symbol", conv->int_curr_symbol);
    dumpsx("currency_symbol", conv->currency_symbol);
    dump("mon_decimal_point", conv->mon_decimal_point);
    dump("mon_thousands_sep", conv->mon_thousands_sep);
    dumpx("mon_grouping", conv->mon_grouping);
    dump("positive_sign", conv->positive_sign);
    dump("negative_sign", conv->negative_sign);

    dumpc("int_frac_digits", conv->int_frac_digits);
    dumpc("frac_digits", conv->frac_digits);
    dumpc("p_cs_precedes", conv->p_cs_precedes);
    dumpc("p_sep_by_space", conv->p_sep_by_space);
    dumpc("n_cs_precedes", conv->n_cs_precedes);
    dumpc("n_sep_by_space", conv->n_sep_by_space);
    dumpc("p_sign_posn", conv->p_sign_posn);
    dumpc("n_sign_posn", conv->n_sign_posn);
    dumpc("int_p_cs_precedes", conv->int_p_cs_precedes);
    dumpc("int_n_cs_precedes", conv->int_n_cs_precedes);
    dumpc("int_p_sep_by_space", conv->int_p_sep_by_space);
    dumpc("int_n_sep_by_space", conv->int_n_sep_by_space);
    dumpc("int_p_sign_posn", conv->int_p_sign_posn);
    dumpc("int_n_sign_posn", conv->int_n_sign_posn);
}

int
main (int argc, char **argv)
{
    for (int i = 1; i < argc; i++) {
	char *name = argv[i];
	if (*name) {
	    char *cp = name + strlen(name) - 1;
	    if (*cp == '/')	/* Trim trailing slash */
		*cp = '\0';
	}

	localedump(name);
    }

    return 0;
}
