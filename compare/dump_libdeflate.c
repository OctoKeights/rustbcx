/* The C half of the comparison with libdeflate (https://github.com/ebiggers/libdeflate) v1.26,
   the one OpenEXR 3.5 comes with in its external/deflate: compresses made-up data at its
   levels 0 and 2 to 9 into zlib streams, as OpenEXR's exr_compress_buffer does, decompresses
   them and damaged copies of them as its exr_uncompress_buffer does, and prints a running
   hash of what comes out. examples/dump.rs prints the same from the Rust port (src/libdeflate),
   and compare/compare.sh compares the two.

     dump libdeflate <seed> <count>   made-up data of eight kinds and many sizes, now and then
                                      several pieces together, compressed at a level; each
                                      stream decompressed whole, into too little room and
                                      into more, damaged, cut short and with bytes after it;
                                      and made-up bytes after a zlib header decompressed */
#include "libdeflate.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* The two programs make the same data from the same seed: a 64-bit linear congruential
   generator, the high half of its state. */
static uint64_t g_state;
static uint32_t Next(void)
{
    g_state = g_state * 6364136223846793005ull + 1442695040888963407ull;
    return (uint32_t)(g_state >> 32);
}

/* FNV-1a, 64 bits. */
static uint64_t g_hash = 14695981039346656037ull;
static void Add(const void *data, size_t size)
{
    const unsigned char *bytes = (const unsigned char *)data;
    size_t i;
    for (i = 0; i < size; ++i)
    {
        g_hash ^= bytes[i];
        g_hash *= 1099511628211ull;
    }
}
static void AddNumber(uint32_t value)
{
    unsigned char bytes[4];
    bytes[0] = (unsigned char)value;
    bytes[1] = (unsigned char)(value >> 8);
    bytes[2] = (unsigned char)(value >> 16);
    bytes[3] = (unsigned char)(value >> 24);
    Add(bytes, 4);
}

static const char *const kWords[16] = { "the", "texture", "block", "of", "and", "compress", "a", "level", "mip", "to", "pixels", "in", "format", "is", "cube", "data" };

/* Made-up data: its length, then its bytes, of one of eight kinds. */
static size_t MakeData(uint8_t *buf)
{
    size_t len = 0, at = 0, i;
    uint32_t kind;
    switch (Next() % 8)
    {
        case 0: len = Next() % 16; break;
        case 1: len = Next() % 300; break;
        case 2:
        case 3: len = Next() % 5000; break;
        case 4:
        case 5: len = Next() % 40000; break;
        case 6: len = 60000 + Next() % 20000; break;
        default: len = Next() % 200000; break;
    }
    kind = Next() % 8;
    switch (kind)
    {
        case 0: /* noise */
            for (i = 0; i < len; ++i)
                buf[i] = (uint8_t)(Next() >> 13);
            break;
        case 1: /* four letters */
            for (i = 0; i < len; ++i)
                buf[i] = (uint8_t)('a' + Next() % 4);
            break;
        case 2: /* words */
            while (at < len)
            {
                const char *word = kWords[Next() % 16];
                for (i = 0; word[i] && at < len; ++i)
                    buf[at++] = (uint8_t)word[i];
                if (at < len)
                    buf[at++] = ' ';
            }
            break;
        case 3: /* runs */
            while (at < len)
            {
                uint8_t byte = (uint8_t)Next();
                uint32_t run = 1 + Next() % 300;
                for (i = 0; i < run && at < len; ++i)
                    buf[at++] = byte;
            }
            break;
        case 4: /* copies of what came before */
            while (at < len)
            {
                if (at < 64 || Next() % 4 == 0)
                    buf[at++] = (uint8_t)(Next() >> 9);
                else
                {
                    size_t reach = at < 40000 ? at : 40000;
                    size_t dist = 1 + Next() % reach;
                    uint32_t n = 3 + Next() % 260;
                    for (i = 0; i < n && at < len; ++i, ++at)
                        buf[at] = buf[at - dist];
                }
            }
            break;
        case 5: /* nothing */
            memset(buf, 0, len);
            break;
        case 6: /* a slope */
            for (i = 0; i < len; ++i)
                buf[i] = (uint8_t)(i / 7 + i % 13);
            break;
        default: /* stretches of noise, of one value and of counting */
            while (at < len)
            {
                uint32_t stretch = 1 + Next() % 2000, what = Next() % 3, value = Next();
                for (i = 0; i < stretch && at < len; ++i, ++at)
                    buf[at] = (uint8_t)(what == 0 ? Next() >> 11 : what == 1 ? value : value + i);
            }
            break;
    }
    return len;
}

/* exr_compress_max_buffer_size of OpenEXR 3.5 (compression.c). */
static size_t Bound(size_t in_bytes)
{
    size_t r = libdeflate_zlib_compress_bound(NULL, in_bytes) + 9, extra = r * 130 / 128;
    if (extra > r)
        r = extra;
    if (r < in_bytes + 1835036)
        r = in_bytes + 1835036;
    return r < 8192 ? 8192 : r;
}

/* exr_uncompress_buffer: 0 for done (and the bytes made), 1 for a corrupt stream, 2 for one
   that makes more bytes than there is room for. */
static uint32_t Uncompress(const uint8_t *in, size_t in_bytes, uint8_t *out, size_t avail, size_t *made)
{
    struct libdeflate_decompressor *d = libdeflate_alloc_decompressor();
    size_t actual_in = 0;
    enum libdeflate_result res = libdeflate_zlib_decompress_ex(d, in, in_bytes, out, avail, &actual_in, made);
    libdeflate_free_decompressor(d);
    if (res == LIBDEFLATE_SUCCESS)
        return actual_in == in_bytes ? 0 : 1;
    if (res == LIBDEFLATE_INSUFFICIENT_SPACE)
        return 2;
    if (res == LIBDEFLATE_SHORT_OUTPUT)
        return 0;
    return 1;
}

static void AddUncompress(const uint8_t *in, size_t in_bytes, size_t avail)
{
    uint8_t *out = (uint8_t *)malloc(avail + 1);
    size_t made = 0;
    const uint32_t result = Uncompress(in, in_bytes, out, avail, &made);
    AddNumber(result);
    if (result == 0)
    {
        AddNumber((uint32_t)made);
        Add(out, made);
    }
    free(out);
}

int main(int argc, char **argv)
{
    static const int kLevels[10] = { 0, 2, 3, 4, 5, 6, 7, 8, 9, 4 };
    uint32_t count, round;
    uint8_t *data = (uint8_t *)malloc(3 * 200000);
    uint8_t *stream = (uint8_t *)malloc(Bound(3 * 200000));
    uint64_t total = 0;
    if (argc < 4 || strcmp(argv[1], "libdeflate") != 0)
    {
        fprintf(stderr, "usage: dump libdeflate <seed> <count>\n");
        return 2;
    }
    g_state = strtoull(argv[2], NULL, 10);
    count = (uint32_t)strtoul(argv[3], NULL, 10);
    for (round = 0; round < count; ++round)
    {
        size_t len = MakeData(data), size, i;
        int level;
        struct libdeflate_compressor *c;
        /* Now and then two or three pieces, for blocks of more than 300000 bytes. */
        if (Next() % 8 == 0)
        {
            const uint32_t more = 1 + Next() % 2;
            for (i = 0; i < more; ++i)
                len += MakeData(data + len);
        }
        level = kLevels[Next() % 10];
        c = libdeflate_alloc_compressor(level);
        size = libdeflate_zlib_compress(c, data, len, stream, Bound(len));
        libdeflate_free_compressor(c);
        total += len;
        AddNumber((uint32_t)len);
        AddNumber((uint32_t)level);
        AddNumber((uint32_t)size);
        Add(stream, size);

        /* Back: into room enough, too little, or more; damaged, cut short, with bytes after. */
        {
            const uint32_t how = Next() % 8, pick = Next(), pick2 = Next();
            size_t room = len, n = size;
            if (how == 1 && len > 0)
                room = len - 1 - pick % (len < 16 ? len : 16);
            else if (how == 2)
                room = len + pick % 100;
            else if (how == 3)
            {
                const uint32_t flips = 1 + pick2 % 3;
                for (i = 0; i < flips; ++i)
                {
                    const uint32_t bit = Next() % (uint32_t)(size * 8);
                    stream[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
                }
            }
            else if (how == 4)
                n = pick % (size + 1);
            else if (how == 5)
            {
                const uint32_t extra = 1 + pick2 % 9;
                for (i = 0; i < extra; ++i)
                    stream[size + i] = (uint8_t)Next();
                n = size + extra;
            }
            else if (how == 6)
                stream[pick % size] = (uint8_t)pick2;
            AddNumber(how);
            AddUncompress(stream, n, room);
        }

        /* Made-up bytes after a zlib header (and a final block of each type, now and then). */
        if (round % 4 == 0)
        {
            const size_t n = 6 + Next() % 400;
            stream[0] = 0x78;
            stream[1] = 0x5e;
            for (i = 2; i < n; ++i)
                stream[i] = (uint8_t)(Next() >> 7);
            if (Next() % 2)
                stream[2] = (uint8_t)((stream[2] & ~7u) | 1u | ((Next() % 3) << 1));
            AddUncompress(stream, n, Next() % 3000);
        }

        if ((round + 1) % 200 == 0 || round + 1 == count)
            printf("libdeflate %u %016llx\n", round + 1, (unsigned long long)g_hash);
    }
    printf("libdeflate bytes: %llu\n", (unsigned long long)total);
    return 0;
}
