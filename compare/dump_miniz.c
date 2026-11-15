/* The C half of the comparison with miniz (https://github.com/richgel999/miniz): runs its
   compressor and its decompressor over made-up data and prints a running hash of what comes
   out. examples/dump.rs prints the same from the Rust port, and compare/compare.sh compares
   the two.

     dump miniz <seed> <count>   made-up data of eight kinds and many sizes, compressed with
                                 flags from every level and strategy and with made-up flags,
                                 in one go and in pieces with flushes between; each stream
                                 decompressed whole, into too little room, damaged, cut short
                                 and with bytes after it; and made-up bytes decompressed */
#include "miniz.h"

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

static uint32_t MakeFlags(void)
{
    uint32_t flags;
    switch (Next() % 4)
    {
        case 0:
        case 1:
        {
            int level = (int)(Next() % 12) - 1;
            int strategy = (Next() % 3 != 0) ? 0 : (int)(Next() % 5);
            int window_bits = (Next() & 1) ? 15 : -15;
            flags = tdefl_create_comp_flags_from_zip_params(level, window_bits, strategy);
            if (Next() & 1)
                flags |= TDEFL_COMPUTE_ADLER32;
            break;
        }
        case 2:
        {
            static const uint32_t kFlags[7] = { TDEFL_WRITE_ZLIB_HEADER, TDEFL_COMPUTE_ADLER32, TDEFL_GREEDY_PARSING_FLAG, TDEFL_RLE_MATCHES, TDEFL_FILTER_MATCHES, TDEFL_FORCE_ALL_STATIC_BLOCKS,
                                                TDEFL_FORCE_ALL_RAW_BLOCKS };
            int k;
            flags = Next() % 4096;
            for (k = 0; k < 7; ++k)
                if ((Next() & 3) == 0)
                    flags |= kFlags[k];
            break;
        }
        default:
            flags = Next() % 8;
            if (Next() & 1)
                flags |= TDEFL_WRITE_ZLIB_HEADER;
            break;
    }
    return flags;
}

typedef struct
{
    uint8_t *bytes;
    size_t size, room;
} Out;

static mz_bool Put(const void *buf, int len, void *user)
{
    Out *out = (Out *)user;
    if (out->size + (size_t)len > out->room)
    {
        out->room = (out->size + (size_t)len) * 2 + 1024;
        out->bytes = (uint8_t *)realloc(out->bytes, out->room);
    }
    memcpy(out->bytes + out->size, buf, (size_t)len);
    out->size += (size_t)len;
    return MZ_TRUE;
}

/* Decompresses with all the input and all the room at once, as tinfl_decompress_mem_to_mem
   does, and adds what came of it to the hash: the status, and when it is done how much of the
   input it took and the bytes. */
static int Decompress(const uint8_t *stream, size_t stream_size, uint8_t *room, size_t room_size, int zlib)
{
    tinfl_decompressor decomp;
    size_t in_size = stream_size, out_size = room_size;
    tinfl_status status;
    tinfl_init(&decomp);
    status = tinfl_decompress(&decomp, stream, &in_size, room, room, &out_size, (zlib ? TINFL_FLAG_PARSE_ZLIB_HEADER : 0) | TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
    AddNumber((uint32_t)(int32_t)status);
    if (status == TINFL_STATUS_DONE)
    {
        AddNumber((uint32_t)in_size);
        AddNumber((uint32_t)out_size);
        Add(room, out_size);
    }
    return (int)status;
}

int main(int argc, char **argv)
{
    uint32_t count, i, k, done = 0, refused = 0;
    uint8_t *data = (uint8_t *)malloc(200000 + 16), *room = (uint8_t *)malloc(200000 + 16), *copy = (uint8_t *)malloc(400000);
    uint64_t in_total = 0, out_total = 0;
    if (argc < 4 || strcmp(argv[1], "miniz") != 0)
    {
        fprintf(stderr, "usage: dump miniz <seed> <count>\n");
        return 2;
    }
    g_state = strtoull(argv[2], NULL, 10);
    count = (uint32_t)strtoul(argv[3], NULL, 10);
    for (i = 0; i < count; ++i)
    {
        const size_t len = MakeData(data);
        const uint32_t flags = MakeFlags();
        const int zlib = (flags & TDEFL_WRITE_ZLIB_HEADER) != 0;
        size_t stream_size = 0, pos;
        uint8_t *stream = (uint8_t *)tdefl_compress_mem_to_heap(data, len, &stream_size, (int)flags);
        Out pieces = { NULL, 0, 0 };
        tdefl_compressor *comp = (tdefl_compressor *)malloc(sizeof(tdefl_compressor));

        AddNumber((uint32_t)stream_size);
        Add(stream, stream_size);
        in_total += len;
        out_total += stream_size;

        /* The checksums, and what mz_compress2 writes for a level. */
        AddNumber((uint32_t)mz_adler32(MZ_ADLER32_INIT, data, len));
        AddNumber((uint32_t)mz_crc32(MZ_CRC32_INIT, data, len));
        if (i % 4 == 0)
        {
            const int level = (int)(Next() % 12) - 1;
            mz_ulong size = mz_compressBound((mz_ulong)len) + 1024;
            uint8_t *packed = (uint8_t *)malloc(size);
            if (mz_compress2(packed, &size, data, (mz_ulong)len, level) != MZ_OK)
                size = 0;
            AddNumber((uint32_t)size);
            Add(packed, size);
            free(packed);
        }

        /* In pieces, with flushes between. */
        tdefl_init(comp, Put, &pieces, (int)flags);
        for (pos = 0; pos < len;)
        {
            size_t n = 1 + Next() % 5000;
            const uint32_t how = Next() % 8;
            if (n > len - pos)
                n = len - pos;
            AddNumber((uint32_t)(int32_t)tdefl_compress_buffer(comp, data + pos, n, how == 0 ? TDEFL_SYNC_FLUSH : how == 1 ? TDEFL_FULL_FLUSH : TDEFL_NO_FLUSH));
            pos += n;
        }
        AddNumber((uint32_t)(int32_t)tdefl_compress_buffer(comp, data + len, 0, TDEFL_FINISH));
        AddNumber((uint32_t)tdefl_get_adler32(comp));
        AddNumber((uint32_t)pieces.size);
        Add(pieces.bytes, pieces.size);
        if (Decompress(pieces.bytes, pieces.size, room, len, zlib) == TINFL_STATUS_DONE)
            ++done;
        else
            ++refused;
        free(comp);
        free(pieces.bytes);

        /* The stream back: whole, into too little room, damaged, cut short, with more after. */
        if (Decompress(stream, stream_size, room, len, zlib) == TINFL_STATUS_DONE)
            ++done;
        else
            ++refused;
        if (len > 0)
            Decompress(stream, stream_size, room, len - 1 - Next() % len, zlib);
        memcpy(copy, stream, stream_size);
        for (k = 1 + Next() % 3; k > 0; --k)
        {
            const uint32_t bit = Next() % (uint32_t)(stream_size * 8);
            copy[bit >> 3] ^= (uint8_t)(1u << (bit & 7));
        }
        if (Decompress(copy, stream_size, room, len + 64, zlib) == TINFL_STATUS_DONE)
            ++done;
        else
            ++refused;
        Decompress(stream, Next() % stream_size, room, len, zlib);
        memcpy(copy, stream, stream_size);
        for (k = 0; k < 4; ++k)
            copy[stream_size + k] = (uint8_t)Next();
        Decompress(copy, stream_size + 4, room, len, zlib);

        /* Made-up bytes as a stream. */
        {
            const size_t garbage = 1 + Next() % 64;
            for (k = 0; k < garbage; ++k)
                copy[k] = (uint8_t)(Next() >> 7);
            if (Next() & 1)
            {
                /* (A zlib header in front, so that it gets past that.) */
                copy[0] = 0x78;
                copy[1] = 0x9C;
            }
            if (Decompress(copy, garbage, room, 4096, copy[0] == 0x78) == TINFL_STATUS_DONE)
                ++done;
            else
                ++refused;
        }
        mz_free(stream);

        if ((i + 1) % 200 == 0 || i + 1 == count)
            printf("miniz %u %016llx\n", i + 1, (unsigned long long)g_hash);
    }
    printf("miniz bytes in: %llu, out: %llu; streams decompressed: %u, refused: %u\n", (unsigned long long)in_total, (unsigned long long)out_total, done, refused);
    free(data);
    free(room);
    free(copy);
    return 0;
}
