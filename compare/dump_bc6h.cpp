// The reference half of the comparison for BC6H: bcdec.h's unpacker (and, built with
// -DWITH_ISPC_TEXCOMP, the packer of Intel's ISPC Texture Compressor). examples/dump.rs prints
// the same from the Rust port, and compare/compare.sh compares the two.
//
//   dump bc6h_unpack <seed> <blocks>   made-up BC6H blocks of every mode (and of none)
//                                      unpacked as unsigned and as signed, to halves and to
//                                      floats

#define BCDEC_IMPLEMENTATION
#include "bcdec.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>

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
    else
        return 2;
    return 0;
}
