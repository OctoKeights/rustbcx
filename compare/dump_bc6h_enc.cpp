// The C++ half of the comparison with bc6h_enc (https://github.com/0xc0de/bc6h_enc), the BC6H
// encoder of DirectXTex as one file: runs it over made-up HDR pixels and prints a running hash
// of the blocks. examples/dump.rs prints the same from the Rust port, and compare/compare.sh
// compares the two.
//
//   dump bc6h_enc <seed> <blocks>   made-up pixels of eight kinds, of both signs, packed as
//                                   signed and as unsigned BC6H
//
// Built with BC6H_SSE_INTRINSICS, the build that does what DirectXTex does: floats to halves
// by the processor (nearest, ties to even) and a pixel's error over red, green and blue.
// (Without it bc6h_enc.h takes the error over a fourth value of each palette colour as well,
// which it never sets.)
#define BC6H_SSE_INTRINSICS
#define BC6H_ENC_IMPLEMENTATION
#include "bc6h_enc.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

namespace
{
// The two programs make the same pixels from the same seed: a 64-bit linear congruential
// generator, the high half of its state.
struct Random
{
    std::uint64_t state;
    std::uint32_t Next()
    {
        state = state * 6364136223846793005ull + 1442695040888963407ull;
        return static_cast<std::uint32_t>(state >> 32);
    }
};

// FNV-1a, 64 bits.
struct Hash
{
    std::uint64_t value = 14695981039346656037ull;
    void Add(const void* data, std::size_t size)
    {
        const auto* bytes = static_cast<const unsigned char*>(data);
        for (std::size_t i = 0; i < size; ++i)
        {
            value ^= bytes[i];
            value *= 1099511628211ull;
        }
    }
};

// A whole number from -range to range, as a float.
float Spread(Random& r, std::uint32_t range)
{
    return static_cast<float>(static_cast<std::int32_t>(r.Next() % (2 * range + 1)) - static_cast<std::int32_t>(range));
}

// Sixteen made-up HDR pixels, four floats each (the fourth is not packed): noise about zero, a
// ramp between two colours, one colour, two colours, values past what a half holds, tiny
// values, a smooth patch, a ramp above zero.
void MakeFloatBlock(Random& r, float px[16][4])
{
    const std::uint32_t kind = r.Next() % 8;
    float a[3], b[3], c[3];
    for (int k = 0; k < 3; ++k)
    {
        a[k] = Spread(r, 8000) * 0.001f;
        b[k] = Spread(r, 8000) * 0.001f;
        c[k] = Spread(r, 500) * 0.001f;
    }
    for (int i = 0; i < 16; ++i)
    {
        px[i][3] = 1.0f;
        for (int k = 0; k < 3; ++k)
        {
            float v;
            switch (kind)
            {
            case 0: v = Spread(r, 2000) * 0.001f; break;
            case 1: v = a[k] + (b[k] - a[k]) * (static_cast<float>(i) * (1.0f / 15.0f)) + Spread(r, 20) * 0.001f; break;
            case 2: v = a[k]; break;
            case 3: v = (r.Next() & 1) ? a[k] : b[k]; break;
            case 4: v = Spread(r, 100000); break;
            case 5: v = Spread(r, 1000) * 0.000001f; break;
            case 6: v = a[k] + static_cast<float>(i % 4) * c[k] + static_cast<float>(i / 4) * (c[k] * 0.5f); break;
            default: v = (a[k] < 0.0f ? -a[k] : a[k]) + 0.25f + static_cast<float>(i) * 0.03125f; break;
            }
            px[i][k] = v;
        }
    }
}

std::uint32_t Mode(const std::uint8_t block[16])
{
    static const int kModes[32] = {0, 1, 2, 10, -1, -1, 3, 11, -1, -1, 4, 12, -1, -1, 5, 13, -1, -1, 6, -1, -1, -1, 7, -1, -1, -1, 8, -1, -1, -1, 9, -1};
    const std::uint32_t bits = (block[0] & 2) ? (block[0] & 31) : (block[0] & 3);
    return kModes[bits] < 0 ? 14 : static_cast<std::uint32_t>(kModes[bits]);
}

void DumpBc6hEnc(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    std::uint32_t modes[2][15] = {};
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        float pixels[16][4];
        MakeFloatBlock(random, pixels);
        std::uint8_t block[16];
        std::memset(block, 0xEE, sizeof block);
        bc6h_enc::EncodeBC6HS(block, pixels);
        hash.Add(block, sizeof block);
        ++modes[0][Mode(block)];
        std::memset(block, 0xEE, sizeof block);
        bc6h_enc::EncodeBC6HU(block, pixels);
        hash.Add(block, sizeof block);
        ++modes[1][Mode(block)];
        if ((i + 1) % 2000 == 0 || i + 1 == blocks)
            std::printf("bc6h_enc %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    for (int s = 0; s < 2; ++s)
    {
        std::printf("bc6h_enc %s blocks by mode:", s == 0 ? "signed" : "unsigned");
        for (int m = 0; m < 15; ++m)
            std::printf(" %u", modes[s][m]);
        std::printf("\n");
    }
}
} // namespace

int main(int argc, char** argv)
{
    if (argc < 4 || std::strcmp(argv[1], "bc6h_enc") != 0)
    {
        std::fprintf(stderr, "usage: dump bc6h_enc <seed> <blocks>\n");
        return 2;
    }
    DumpBc6hEnc(std::strtoull(argv[2], nullptr, 10), static_cast<std::uint32_t>(std::strtoul(argv[3], nullptr, 10)));
    return 0;
}
