/* The C half of the comparison of DWAA and DWAB (src/dwa) with OpenEXR's OpenEXRCore v3.5.4
   (https://github.com/AcademySoftwareFoundation/openexr): makes up blocks of an image (their
   channels, size and values), has OpenEXRCore write each as the one block of a file of
   lines or of one tile, takes the block back out of the file, decompresses it and damaged
   copies of it as OpenEXRCore does, and prints a running hash of what comes out.
   examples/dump.rs prints the same from the Rust port, and compare/compare.sh compares the
   two.

     dump dwa_write <seed> <count>   made-up blocks of one to six channels (of halves, floats
                                     and integers, named as DWA's rules have them and not)
                                     compressed as DWAA or DWAB at a made-up level, and back
     dump dwa_read <seed> <count>    those blocks, compressed, damaged in one of eleven ways,
                                     decompressed

   OpenEXRCore picks its code for DWA by what the machine has: on x86-64 with AVX and F16C
   (any since about 2011) its AVX inverse DCT and F16C conversions, which the port is of. The
   dump says which it found, and on another machine the blocks differ from the port's. */
#include "openexr.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* (Not exported; the static library has them.) */
exr_result_t internal_exr_undo_dwaa (exr_decode_pipeline_t* decode, const void* compressed_data, uint64_t comp_buf_size, void* uncompressed_data, uint64_t uncompressed_size);

/* The two programs make the same data from the same seed: a 64-bit linear congruential
   generator, the high half of its state. */
static uint64_t g_state;
static uint32_t Next (void)
{
    g_state = g_state * 6364136223846793005ull + 1442695040888963407ull;
    return (uint32_t) (g_state >> 32);
}

/* FNV-1a, 64 bits. */
static uint64_t g_hash = 14695981039346656037ull;
static void     Add (const void* data, size_t size)
{
    const unsigned char* bytes = (const unsigned char*) data;
    for (size_t i = 0; i < size; ++i)
    {
        g_hash ^= bytes[i];
        g_hash *= 1099511628211ull;
    }
}
static void AddNumber (uint32_t value)
{
    unsigned char bytes[4] = { (unsigned char) value, (unsigned char) (value >> 8), (unsigned char) (value >> 16), (unsigned char) (value >> 24) };
    Add (bytes, 4);
}

/* A float as a half: rounded to nearest, ties to even (the generator's own, not
   OpenEXRCore's: the values are what they are). */
static uint16_t ToHalf (float f)
{
    uint32_t bits;
    memcpy (&bits, &f, 4);
    uint32_t sign = (bits >> 16) & 0x8000, ui = bits & 0x7fffffff;
    if (ui > 0x7f800000) return (uint16_t) (sign | 0x7e00 | ((ui >> 13) & 0x3ff));
    if (ui == 0x7f800000) return (uint16_t) (sign | 0x7c00);
    int32_t  exp = (int32_t) (ui >> 23);
    uint32_t mant = ui & 0x7fffff;
    if (exp >= 113)
    {
        uint32_t h = ((uint32_t) (exp - 112) << 10) | (mant >> 13), rest = mant & 0x1fff;
        if (rest > 0x1000 || (rest == 0x1000 && (h & 1))) ++h;
        if (h >= 0x7c00) h = 0x7c00;
        return (uint16_t) (sign | h);
    }
    if (exp < 102) return (uint16_t) sign;
    uint32_t m = mant | 0x800000, shift = (uint32_t) (126 - exp), h = m >> shift, rest = m & ((1u << shift) - 1), halfway = 1u << (shift - 1);
    if (rest > halfway || (rest == halfway && (h & 1))) ++h;
    return (uint16_t) (sign | h);
}

static const char* const kNames[24] = { "R", "G", "B", "A", "Y", "BY", "RY", "Z", "r", "g", "b", "a", "red", "green", "blue", "diffuse.R", "diffuse.G", "diffuse.B", "diffuse.A", "spec.R", "spec.B", "left.Y", "mask", "N.x" };

typedef struct
{
    int      compression; /* 8 DWAA, 9 DWAB */
    int      tiled;
    int      width, lines;
    float    level;
    int      count;
    int      names[6];
    int      types[6]; /* 0 integers, 1 halves, 2 floats */
    int      linear[6];
    uint8_t* planes[6]; /* each channel's lines, one after the other */
} Block;

static int CompareNames (const void* a, const void* b)
{
    return strcmp (kNames[*(const int*) a], kNames[*(const int*) b]);
}

/* A made-up value of a channel at (x, y), of one of eight kinds. */
typedef struct
{
    int      kind;
    uint32_t a, b, c;
} Values;

static float Value (const Values* v, int x, int y)
{
    switch (v->kind)
    {
        case 0: return (float) ((uint32_t) (x * (int) v->a + y * (int) v->b + (int) v->c) % 4096u) / 1000.f - 1.f;
        case 1: return (float) (Next () % 20000u) / 5000.f - 2.f;
        case 2: return (float) v->a / 100.f;
        case 3: return Next () % 16 == 0 ? (float) (Next () % 1000u) / 50.f : 0.f;
        case 6: return (float) (((uint32_t) (x / 8 + y / 8) + v->a) % 3u) * 0.5f;
        default: {
            float f = (float) (Next () % 60000u) + 0.5f;
            return Next () % 2 ? f : -f;
        }
    }
}

static void MakeBlock (Block* b)
{
    b->compression = Next () % 2 ? 8 : 9;
    b->tiled       = Next () % 4 == 0;
    switch (Next () % 8)
    {
        case 0: b->width = 1 + (int) (Next () % 8); break;
        case 1:
        case 2:
        case 3:
        case 4: b->width = 1 + (int) (Next () % 64); break;
        case 5:
        case 6: b->width = 1 + (int) (Next () % 160); break;
        default: b->width = 1 + (int) (Next () % 400); break;
    }
    int most = b->compression == 8 ? 32 : 256;
    int few  = Next () % 8 < 6;
    b->lines = 1 + (int) (Next () % (uint32_t) (few ? (most < 40 ? most : 40) : most));
    switch (Next () % 8)
    {
        case 4: b->level = 0.f; break;
        case 5: b->level = (float) (Next () % 300u) + (float) (Next () % 100u) / 100.f; break;
        case 6: b->level = (float) (Next () % 100000u) / 7.f; break;
        case 7: b->level = (float) (Next () % 2000u) / 10.f; break;
        default: b->level = 45.f; break;
    }
    b->count = 1 + (int) (Next () % 6);
    for (int c = 0; c < b->count; ++c)
    {
        for (;;)
        {
            int name = (int) (Next () % 24), used = 0;
            for (int k = 0; k < c; ++k)
                used |= b->names[k] == name;
            if (!used)
            {
                b->names[c] = name;
                break;
            }
        }
    }
    qsort (b->names, (size_t) b->count, sizeof (int), CompareNames);
    for (int c = 0; c < b->count; ++c)
    {
        uint32_t t   = Next () % 10;
        b->types[c]  = t < 6 ? 1 : t < 9 ? 2 : 0;
        b->linear[c] = Next () % 8 == 0;
        int    bpe   = b->types[c] == 1 ? 2 : 4;
        Values v;
        v.kind       = (int) (Next () % 8);
        v.a          = Next () % 32;
        v.b          = Next () % 32;
        v.c          = Next () % 1000;
        b->planes[c] = malloc ((size_t) b->width * (size_t) b->lines * (size_t) bpe);
        for (int y = 0; y < b->lines; ++y)
        {
            for (int x = 0; x < b->width; ++x)
            {
                uint8_t* at = b->planes[c] + ((size_t) y * (size_t) b->width + (size_t) x) * (size_t) bpe;
                if (b->types[c] == 0)
                {
                    uint32_t u = v.kind < 4 ? Next () % 256u : Next ();
                    memcpy (at, &u, 4);
                    continue;
                }
                uint32_t bits;
                float    f;
                if (v.kind == 4 || v.kind == 5)
                {
                    /* Any bits at all, but none that are not finite in a linear channel
                       (whose values are taken as they are, NaNs and all). */
                    bits = Next ();
                    if (b->types[c] == 1 || v.kind == 4)
                    {
                        uint16_t h = (uint16_t) bits;
                        if (b->linear[c] && (h & 0x7c00) == 0x7c00) h &= 0xfbff;
                        if (b->types[c] == 1)
                        {
                            memcpy (at, &h, 2);
                            continue;
                        }
                        /* (A half's value, as a float.) */
                        uint32_t sign = (uint32_t) (h >> 15) << 31, e = (h >> 10) & 0x1f, m = h & 0x3ff;
                        if (e == 0x1f) bits = sign | 0x7f800000 | (m << 13);
                        else if (e != 0) bits = sign | ((e + 112) << 23) | (m << 13);
                        else
                        {
                            float d = (float) m / 16777216.f;
                            memcpy (&bits, &d, 4);
                            bits |= sign;
                        }
                    }
                    else if (b->linear[c] && (bits & 0x7f800000) == 0x7f800000)
                        bits = 0;
                    memcpy (at, &bits, 4);
                    continue;
                }
                f = Value (&v, x, y);
                if (b->types[c] == 1)
                {
                    uint16_t h = ToHalf (f);
                    memcpy (at, &h, 2);
                }
                else
                    memcpy (at, &f, 4);
            }
        }
    }
}

static void FreeBlock (Block* b)
{
    for (int c = 0; c < b->count; ++c)
        free (b->planes[c]);
}

/* The block as it is in a file: its lines one after the other, each with its channels one
   after the other. */
static uint8_t* RawBlock (const Block* b, size_t* size)
{
    size_t line = 0;
    for (int c = 0; c < b->count; ++c)
        line += (size_t) b->width * (b->types[c] == 1 ? 2u : 4u);
    uint8_t* raw = malloc (line * (size_t) b->lines + 1);
    size_t   at  = 0;
    for (int y = 0; y < b->lines; ++y)
        for (int c = 0; c < b->count; ++c)
        {
            size_t row = (size_t) b->width * (b->types[c] == 1 ? 2u : 4u);
            memcpy (raw + at, b->planes[c] + row * (size_t) y, row);
            at += row;
        }
    *size = at;
    return raw;
}

/* A file in memory. */
typedef struct
{
    uint8_t* data;
    size_t   size, room;
} Memory;

static int64_t WriteMemory (exr_const_context_t ctxt, void* user, const void* buffer, uint64_t sz, uint64_t offset, exr_stream_error_func_ptr_t error_cb)
{
    Memory* m = user;
    (void) ctxt;
    (void) error_cb;
    if (offset + sz > m->room)
    {
        m->room = (size_t) (offset + sz) * 2;
        m->data = realloc (m->data, m->room);
    }
    memcpy (m->data + offset, buffer, sz);
    if (offset + sz > m->size) m->size = (size_t) (offset + sz);
    return (int64_t) sz;
}

static int64_t ReadMemory (exr_const_context_t ctxt, void* user, void* buffer, uint64_t sz, uint64_t offset, exr_stream_error_func_ptr_t error_cb)
{
    Memory* m = user;
    (void) ctxt;
    (void) error_cb;
    if (offset >= m->size) return 0;
    if (offset + sz > m->size) sz = m->size - offset;
    memcpy (buffer, m->data + offset, sz);
    return (int64_t) sz;
}

static int64_t SizeMemory (exr_const_context_t ctxt, void* user)
{
    (void) ctxt;
    return (int64_t) ((Memory*) user)->size;
}

static void Quiet (exr_const_context_t ctxt, exr_result_t code, const char* msg)
{
    (void) ctxt;
    (void) code;
    (void) msg;
}

static void Check (exr_result_t rv, const char* what)
{
    if (rv != EXR_ERR_SUCCESS)
    {
        fprintf (stderr, "OpenEXRCore: %s: %d\n", what, (int) rv);
        exit (1);
    }
}

/* The block written by OpenEXRCore as the one block of a file, which is opened again for
   reading (and the block's place in it found). */
typedef struct
{
    Memory             file;
    exr_context_t      read;
    exr_chunk_info_t   chunk;
} Written;

static void Write (const Block* b, Written* w)
{
    exr_context_initializer_t init = EXR_DEFAULT_CONTEXT_INITIALIZER;
    exr_context_t             ctxt;
    int                       part;
    memset (w, 0, sizeof (*w));
    init.user_data        = &w->file;
    init.write_fn         = WriteMemory;
    init.error_handler_fn = Quiet;
    Check (exr_start_write (&ctxt, "block", EXR_WRITE_FILE_DIRECTLY, &init), "start_write");
    Check (exr_add_part (ctxt, "block", b->tiled ? EXR_STORAGE_TILED : EXR_STORAGE_SCANLINE, &part), "add_part");
    Check (exr_initialize_required_attr_simple (ctxt, part, b->width, b->lines, (exr_compression_t) b->compression), "attributes");
    if (b->tiled) Check (exr_set_tile_descriptor (ctxt, part, (uint32_t) b->width, (uint32_t) b->lines, EXR_TILE_ONE_LEVEL, EXR_TILE_ROUND_DOWN), "tiles");
    for (int c = 0; c < b->count; ++c)
        Check (exr_add_channel (ctxt, part, kNames[b->names[c]], (exr_pixel_type_t) b->types[c], b->linear[c] ? EXR_PERCEPTUALLY_LINEAR : EXR_PERCEPTUALLY_LOGARITHMIC, 1, 1), "add_channel");
    Check (exr_set_dwa_compression_level (ctxt, part, b->level), "level");
    Check (exr_write_header (ctxt), "write_header");

    exr_chunk_info_t      cinfo;
    exr_encode_pipeline_t encode;
    if (b->tiled) Check (exr_write_tile_chunk_info (ctxt, part, 0, 0, 0, 0, &cinfo), "tile_chunk_info");
    else
        Check (exr_write_scanline_chunk_info (ctxt, part, 0, &cinfo), "scanline_chunk_info");
    Check (exr_encoding_initialize (ctxt, part, &cinfo, &encode), "encoding_initialize");
    for (int c = 0; c < encode.channel_count; ++c)
    {
        int bpe                                = b->types[c] == 1 ? 2 : 4;
        encode.channels[c].encode_from_ptr     = b->planes[c];
        encode.channels[c].user_bytes_per_element = (int16_t) bpe;
        encode.channels[c].user_data_type      = (uint16_t) b->types[c];
        encode.channels[c].user_pixel_stride   = bpe;
        encode.channels[c].user_line_stride    = bpe * b->width;
    }
    Check (exr_encoding_choose_default_routines (ctxt, part, &encode), "encoding_choose_default_routines");
    Check (exr_encoding_run (ctxt, part, &encode), "encoding_run");
    Check (exr_encoding_destroy (ctxt, &encode), "encoding_destroy");
    Check (exr_finish (&ctxt), "finish");

    exr_context_initializer_t rinit = EXR_DEFAULT_CONTEXT_INITIALIZER;
    rinit.user_data                 = &w->file;
    rinit.read_fn                   = ReadMemory;
    rinit.size_fn                   = SizeMemory;
    rinit.error_handler_fn          = Quiet;
    Check (exr_start_read (&w->read, "block", &rinit), "start_read");
    if (b->tiled) Check (exr_read_tile_chunk_info (w->read, 0, 0, 0, 0, 0, &w->chunk), "read_tile_chunk_info");
    else
        Check (exr_read_scanline_chunk_info (w->read, 0, 0, &w->chunk), "read_scanline_chunk_info");
}

static uint64_t g_decompressed, g_refused;

/* The block `data` decompressed as OpenEXRCore does it, into lines of zeros: its answer,
   and the lines. */
static void Uncompress (Written* w, const uint8_t* data, size_t size)
{
    exr_decode_pipeline_t decode;
    Check (exr_decoding_initialize (w->read, 0, &w->chunk, &decode), "decoding_initialize");
    size_t   out_size        = (size_t) w->chunk.unpacked_size;
    uint8_t* out             = calloc (out_size + 1, 1);
    decode.unpacked_buffer   = out;
    decode.unpacked_alloc_size = out_size;
    exr_result_t rv          = internal_exr_undo_dwaa (&decode, data, size, out, out_size);
    decode.unpacked_buffer   = NULL;
    decode.unpacked_alloc_size = 0;
    Check (exr_decoding_destroy (w->read, &decode), "decoding_destroy");
    AddNumber ((uint32_t) rv);
    Add (out, out_size);
    if (rv == EXR_ERR_SUCCESS) ++g_decompressed;
    else
        ++g_refused;
    free (out);
}

static void Finish (Written* w)
{
    exr_finish (&w->read);
    free (w->file.data);
}

static uint64_t Wide (void)
{
    uint64_t low = Next ();
    return low | ((uint64_t) Next () << 32);
}

static void PutCounter (uint8_t* data, int i, uint64_t value)
{
    for (int k = 0; k < 8; ++k)
        data[8 * i + k] = (uint8_t) (value >> (8 * k));
}

static uint64_t GetCounter (const uint8_t* data, int i)
{
    uint64_t value = 0;
    for (int k = 0; k < 8; ++k)
        value |= (uint64_t) data[8 * i + k] << (8 * k);
    return value;
}

/* A copy of the block damaged in one of eleven ways (or not): its length. */
static size_t Damage (uint8_t* data, size_t size, size_t room)
{
    switch (Next () % 11)
    {
        case 10: /* made-up rules for the channels in place of its own */
            if (size >= 90 && GetCounter (data, 0) == 2)
            {
                size_t   old  = (size_t) data[88] | ((size_t) data[89] << 8);
                if (old < 2 || 88 + old > size) return size;
                uint8_t  rules[256];
                size_t   n    = 2;
                uint32_t many = 1 + Next () % 6;
                for (uint32_t k = 0; k < many; ++k)
                {
                    const char* suffix = kNames[Next () % 15];
                    size_t      len    = strlen (suffix);
                    memcpy (rules + n, suffix, len + 1);
                    n += len + 1;
                    uint32_t csc = Next () % 4, scheme = Next () % 3, nocase = Next () % 2;
                    rules[n++] = (uint8_t) ((csc << 4) | (scheme << 2) | nocase);
                    rules[n++] = (uint8_t) (Next () % 3);
                }
                rules[0] = (uint8_t) n;
                rules[1] = 0;
                size_t rest = size - 88 - old;
                if (88 + n + rest > room) return size;
                memmove (data + 88 + n, data + 88 + old, rest);
                memcpy (data + 88, rules, n);
                return 88 + n + rest;
            }
            return size;
        case 1: { /* bytes changed */
            uint32_t n = 1 + Next () % 4;
            for (uint32_t k = 0; k < n && size > 0; ++k)
            {
                size_t at = Next () % size;
                data[at] ^= (uint8_t) (1 + Next () % 255);
            }
            return size;
        }
        case 2: /* cut short */
            return size > 0 ? Next () % size : 0;
        case 3: /* a counter changed */
            if (size >= 88)
            {
                int      i = (int) (Next () % 11);
                uint64_t value;
                switch (Next () % 3)
                {
                    case 0: value = Next () % 2000; break;
                    case 1: value = GetCounter (data, i) + Next () % 5 - 2; break;
                    default: value = Wide (); break;
                }
                PutCounter (data, i, value);
            }
            return size;
        case 4: /* the rules changed */
            if (size > 88)
            {
                size_t span = size - 88 < 40 ? size - 88 : 40;
                size_t at = 88 + Next () % span;
                data[at]  = (uint8_t) Next ();
            }
            return size;
        case 5: { /* bytes after it */
            uint32_t n = Next () % 20;
            for (uint32_t k = 0; k < n && size < room; ++k)
                data[size++] = (uint8_t) Next ();
            return size;
        }
        case 6: { /* made-up counters and bytes */
            size_t n = 88 + Next () % 300;
            if (n > room) n = room;
            for (size_t k = 0; k < n; ++k)
                data[k] = (uint8_t) Next ();
            for (int i = 0; i < 11 && n >= 88; ++i)
                PutCounter (data, i, i == 0 ? Next () % 3 : Next () % 400);
            return n;
        }
        case 7: /* bytes changed past the counters and rules */
            if (size > 120)
            {
                uint32_t n = 1 + Next () % 3;
                for (uint32_t k = 0; k < n; ++k)
                {
                    size_t at = 120 + Next () % (size - 120);
                    data[at] ^= (uint8_t) (1 + Next () % 255);
                }
            }
            return size;
        case 8: /* an older version, with the rules taken for data */
            if (size >= 88) PutCounter (data, 0, Next () % 2);
            return size;
        case 9: /* the AC compression changed */
            if (size >= 88) PutCounter (data, 10, Next () % 3);
            return size;
        default: return size;
    }
}

int main (int argc, char** argv)
{
    if (argc < 4)
    {
        fprintf (stderr, "usage: dump dwa_write|dwa_read <seed> <count>\n");
        return 2;
    }
    const char* what  = argv[1];
    g_state           = strtoull (argv[2], NULL, 10);
    long count        = strtol (argv[3], NULL, 10);
    int  read         = !strcmp (what, "dwa_read");
    if (!read && strcmp (what, "dwa_write"))
    {
        fprintf (stderr, "unknown dump %s\n", what);
        return 2;
    }
    __builtin_cpu_init ();
    if (!__builtin_cpu_supports ("avx") || !__builtin_cpu_supports ("f16c")) fprintf (stderr, "%s: this machine has no AVX and F16C: OpenEXRCore uses other code for DWA, which the port is not of\n", what);

    uint64_t raw_total = 0, block_total = 0, stored = 0;
    for (long i = 0; i < count; ++i)
    {
        Block b;
        MakeBlock (&b);
        size_t   raw_size;
        uint8_t* raw = RawBlock (&b, &raw_size);
        Written  w;
        Write (&b, &w);
        size_t   size  = (size_t) w.chunk.packed_size;
        size_t   room  = size + 400;
        uint8_t* block = malloc (room);
        memcpy (block, w.file.data + w.chunk.data_offset, size);
        raw_total += raw_size;
        block_total += size;
        stored += size == raw_size;
        if (!read)
        {
            AddNumber ((uint32_t) raw_size);
            Add (raw, raw_size);
            AddNumber ((uint32_t) size);
            Add (block, size);
            Uncompress (&w, block, size);
        }
        else
        {
            size = Damage (block, size, room);
            AddNumber ((uint32_t) size);
            Uncompress (&w, block, size);
        }
        free (block);
        Finish (&w);
        free (raw);
        FreeBlock (&b);
        if ((i + 1) % 200 == 0 || i + 1 == count) printf ("%s %ld %016llx\n", what, i + 1, (unsigned long long) g_hash);
    }
    printf ("%s bytes: %llu as %llu, %llu blocks stored as they are; decompressed: %llu, refused: %llu\n", what, (unsigned long long) raw_total, (unsigned long long) block_total, (unsigned long long) stored, (unsigned long long) g_decompressed, (unsigned long long) g_refused);
    return 0;
}
