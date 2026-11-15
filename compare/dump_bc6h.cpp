// The reference half of the comparison for BC6H: bcdec.h's unpacker (and, built with
// -DWITH_ISPC_TEXCOMP, the packer of Intel's ISPC Texture Compressor). examples/dump.rs prints
// the same from the Rust port, and compare/compare.sh compares the two.
//
//   dump bc6h_unpack <seed> <blocks>   made-up BC6H blocks of every mode (and of none)
//                                      unpacked as unsigned and as signed, to halves and to
//                                      floats
//   dump bc6h_pack <seed> <blocks>     made-up HDR pixels packed with each of the packer's
//                                      five profiles, and unpacked again (only in a build
//                                      with -DWITH_ISPC_TEXCOMP and the kernel's ISPC object)

#define BCDEC_IMPLEMENTATION
#include "bcdec.h"

#if WITH_ISPC_TEXCOMP
#include "ispc_texcomp.h"
#endif

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

namespace
{
// The two programs make the same blocks from the same seed: a 64-bit linear congruential
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
        const unsigned char* bytes = static_cast<const unsigned char*>(data);
        for (std::size_t i = 0; i < size; ++i)
        {
            value ^= bytes[i];
            value *= 1099511628211ull;
        }
    }
};

// The first bits of the fourteen modes of BC6H.
const std::uint8_t kModeBits[14] = {0x00, 0x01, 0x02, 0x06, 0x0a, 0x0e, 0x12, 0x16, 0x1a, 0x1e, 0x03, 0x07, 0x0b, 0x0f};

void DumpUnpack(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    std::uint32_t black = 0;
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        std::uint8_t block[16];
        for (int k = 0; k < 16; ++k)
            block[k] = static_cast<std::uint8_t>(random.Next());
        // Half of them of a mode in turn; a few with only small values in them.
        if (i % 2 == 0)
        {
            const std::uint8_t bits = kModeBits[(i / 2) % 14];
            block[0] = static_cast<std::uint8_t>((bits < 2) ? ((block[0] & 0xfc) | bits) : ((block[0] & 0xe0) | bits));
        }
        if (i % 16 == 5)
            for (int k = 1; k < 10; ++k)
                block[k] &= static_cast<std::uint8_t>(random.Next());
        for (int is_signed = 0; is_signed < 2; ++is_signed)
        {
            std::uint16_t halves[48];
            bcdec_bc6h_half(block, halves, 12, is_signed);
            hash.Add(halves, sizeof(halves));
            float floats[48];
            bcdec_bc6h_float(block, floats, 12, is_signed);
            hash.Add(floats, sizeof(floats));
            bool all_zero = true;
            for (int k = 0; k < 48; ++k)
                all_zero = all_zero && halves[k] == 0;
            black += all_zero ? 1 : 0;
        }
    }
    std::printf("bc6h_unpack %016llx\n", static_cast<unsigned long long>(hash.value));
    std::printf("bc6h_unpack blocks unpacked to black: %u\n", black);
}
#if WITH_ISPC_TEXCOMP
// Sixteen made-up HDR pixels, four halves each (the fourth is not packed): noise over all the
// halves, values near one, a ramp between two colours, one colour, tiny values, two colours,
// a smooth patch, very bright values.
void MakeHdrBlock(Random& r, std::uint16_t px[64])
{
    const std::uint32_t kind = r.Next() % 8;
    auto half = [&r](std::uint32_t top) { return static_cast<std::uint16_t>(r.Next() % (top + 1)); };
    switch (kind)
    {
    case 0:
        for (int i = 0; i < 64; ++i)
            px[i] = half(0x7bff);
        break;
    case 1:
        for (int i = 0; i < 64; ++i)
            px[i] = static_cast<std::uint16_t>(0x3800 + r.Next() % 0x800);
        break;
    case 2:
    {
        std::uint16_t a[3], b[3];
        for (int c = 0; c < 3; ++c)
            a[c] = half(0x7bff), b[c] = half(0x7bff);
        for (int i = 0; i < 16; ++i)
        {
            const std::uint32_t t = r.Next() % 256;
            for (int c = 0; c < 3; ++c)
                px[i * 4 + c] = static_cast<std::uint16_t>((a[c] * (255 - t) + b[c] * t) / 255);
            px[i * 4 + 3] = 0x3c00;
        }
        break;
    }
    case 3:
    {
        std::uint16_t a[3];
        for (int c = 0; c < 3; ++c)
            a[c] = half(0x7bff);
        for (int i = 0; i < 16; ++i)
        {
            for (int c = 0; c < 3; ++c)
                px[i * 4 + c] = a[c];
            px[i * 4 + 3] = 0x3c00;
        }
        break;
    }
    case 4:
        for (int i = 0; i < 64; ++i)
            px[i] = half(40);
        break;
    case 5:
    {
        std::uint16_t a[2][3];
        for (int k = 0; k < 2; ++k)
            for (int c = 0; c < 3; ++c)
                a[k][c] = half(0x7bff);
        for (int i = 0; i < 16; ++i)
        {
            const int k = static_cast<int>(r.Next() % 2);
            for (int c = 0; c < 3; ++c)
                px[i * 4 + c] = a[k][c];
            px[i * 4 + 3] = 0;
        }
        break;
    }
    case 6:
    {
        const std::uint16_t base = half(0x7000);
        for (int i = 0; i < 64; ++i)
            px[i] = static_cast<std::uint16_t>(base + r.Next() % 64);
        break;
    }
    default:
        for (int i = 0; i < 64; ++i)
            px[i] = static_cast<std::uint16_t>(0x7000 + r.Next() % 0xbff);
        break;
    }
}

void DumpPack(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    // The blocks side by side, 64 to a row, as the packer takes them.
    const std::uint32_t per_row = 64, rows = (blocks + per_row - 1) / per_row;
    std::vector<std::uint16_t> surface(static_cast<std::size_t>(rows) * 4 * per_row * 4 * 4, 0);
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        std::uint16_t px[64];
        MakeHdrBlock(random, px);
        const std::uint32_t bx = i % per_row, by = i / per_row;
        for (std::uint32_t y = 0; y < 4; ++y)
            for (std::uint32_t x = 0; x < 4; ++x)
                for (std::uint32_t c = 0; c < 4; ++c)
                    surface[(static_cast<std::size_t>(by * 4 + y) * per_row * 4 + bx * 4 + x) * 4 + c] = px[(y * 4 + x) * 4 + c];
    }
    rgba_surface source;
    source.ptr = reinterpret_cast<std::uint8_t*>(surface.data());
    source.width = static_cast<std::int32_t>(per_row * 4);
    source.height = static_cast<std::int32_t>(rows * 4);
    source.stride = static_cast<std::int32_t>(per_row * 4 * 8);
    std::vector<std::uint8_t> packed(static_cast<std::size_t>(rows) * per_row * 16);
    static const char* const kNames[5] = {"veryfast", "fast", "basic", "slow", "veryslow"};
    std::uint32_t modes[15] = {};
    for (int profile = 0; profile < 5; ++profile)
    {
        bc6h_enc_settings settings;
        switch (profile)
        {
        case 0: GetProfile_bc6h_veryfast(&settings); break;
        case 1: GetProfile_bc6h_fast(&settings); break;
        case 2: GetProfile_bc6h_basic(&settings); break;
        case 3: GetProfile_bc6h_slow(&settings); break;
        default: GetProfile_bc6h_veryslow(&settings); break;
        }
        CompressBlocksBC6H(&source, packed.data(), &settings);
        Hash hash;
        for (std::uint32_t i = 0; i < blocks; ++i)
        {
            const std::uint8_t* block = &packed[static_cast<std::size_t>(i) * 16];
            hash.Add(block, 16);
            std::uint16_t halves[48];
            bcdec_bc6h_half(block, halves, 12, 0);
            hash.Add(halves, sizeof(halves));
            const std::uint8_t bits = (block[0] & 3) < 2 ? (block[0] & 3) : (block[0] & 31);
            int mode = 14;
            for (int m = 0; m < 14; ++m)
                if (kModeBits[m] == bits)
                    mode = m;
            ++modes[mode];
        }
        std::printf("%s %016llx\n", kNames[profile], static_cast<unsigned long long>(hash.value));
    }
    std::printf("blocks by mode:");
    for (int m = 0; m < 15; ++m)
        std::printf(" %u", modes[m]);
    std::printf("\n");
}
#endif
} // namespace

int main(int argc, char** argv)
{
    if (argc < 4)
    {
        std::fprintf(stderr, "usage: dump <what> <seed> <blocks>   (see the top of dump_bc6h.cpp)\n");
        return 2;
    }
    const std::uint64_t seed = std::strtoull(argv[2], nullptr, 10);
    const std::uint32_t blocks = static_cast<std::uint32_t>(std::strtoul(argv[3], nullptr, 10));
    if (std::strcmp(argv[1], "bc6h_unpack") == 0)
        DumpUnpack(seed, blocks);
#if WITH_ISPC_TEXCOMP
    else if (std::strcmp(argv[1], "bc6h_pack") == 0)
        DumpPack(seed, blocks);
#endif
    else
        return 2;
    return 0;
}
