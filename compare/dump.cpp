// The C++ half of the comparison with bc7enc_rdo: runs the library over made-up blocks and
// prints a running hash of what comes out. examples/dump.rs prints the same from the Rust port,
// and compare/compare.sh compares the two.
//
//   dump bc7decomp <seed> <blocks>   BC7 blocks of every mode (and of none) unpacked
//   dump unpack <seed> <blocks>      BC1, BC3, BC4 and BC5 blocks unpacked every way, and solid
//                                    BC1 blocks made
//   dump bc1 <seed> <blocks>         made-up pixels packed as BC1 at every level, with every
//                                    flag, for each way of interpolating, and unpacked again
//   dump bc345 <seed> <blocks>       the same pixels packed as BC3, BC4 and BC5, each way
//   dump bc7enc <seed> <blocks>      the same pixels packed as BC7 with every setting varied,
//                                    and unpacked again
//   dump bc7block <seed> <blocks>    BC7 blocks of every mode written from made-up endpoints
//                                    and selectors, and unpacked again
//   dump rdo <seed> <blocks>         made-up images of every shape packed whole by
//                                    rdo_bc_encoder in each of its formats, with and without
//                                    the rate-distortion pass, every setting varied; BC7 with
//                                    bc7enc
//   dump rdo_bc7e <seed> <blocks>    the same, BC7 with bc7e for about half the images (only
//                                    in a build with -DSUPPORT_BC7E=1 and the ISPC object)
//   dump ert <seed> <blocks>         runs of made-up blocks, many of them near copies of the
//                                    blocks before, packed in one of five formats and put
//                                    through the entropy reduction transform, its settings
//                                    varied

// (bc7decomp.h, bc7enc.h, ert.h and rgbcx.h come with this one; bc7enc.h cannot be included
// twice.)
#include "rdo_bc_encoder.h"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <vector>

#include <fcntl.h>
#include <unistd.h>

// bc7enc.cpp's own, which its header leaves out.
struct bc7_optimization_results
{
    uint32_t m_mode;
    uint32_t m_partition;
    uint8_t m_selectors[16];
    uint8_t m_alpha_selectors[16];
    color_rgba m_low[3];
    color_rgba m_high[3];
    uint32_t m_pbits[3][2];
    uint32_t m_rotation;
    uint32_t m_index_selector;
};
void encode_bc7_block(void* pBlock, const bc7_optimization_results* pResults);

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

void DumpBc7Decomp(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    std::uint32_t unpacked[9] = {};
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        std::uint8_t block[16];
        for (int k = 0; k < 16; ++k)
            block[k] = static_cast<std::uint8_t>(random.Next());
        // Every third block is of a mode chosen in turn (8: of none); the rest are as they fell.
        if (i % 3 == 0)
        {
            const std::uint32_t mode = (i / 3) % 9;
            block[0] = mode == 8 ? std::uint8_t{0} : static_cast<std::uint8_t>((block[0] & ~((2u << mode) - 1)) | (1u << mode));
        }
        bc7decomp::color_rgba pixels[16];
        std::memset(pixels, 0xEE, sizeof pixels);
        const bool ok = bc7decomp::unpack_bc7(block, pixels);
        std::uint32_t mode = 0;
        while (mode < 8 && !(block[0] & (1u << mode)))
            ++mode;
        unpacked[mode] += ok ? 1 : 0;
        const std::uint8_t flag = ok ? 1 : 0;
        hash.Add(&flag, 1);
        hash.Add(pixels, sizeof pixels);
        if ((i + 1) % 100000 == 0 || i + 1 == blocks)
            std::printf("bc7decomp %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    std::printf("bc7decomp unpacked by mode:");
    for (std::uint32_t mode = 0; mode < 9; ++mode)
        std::printf(" %u", unpacked[mode]);
    std::printf("\n");
}
// Sixteen made-up pixels of one of eight kinds, the same in both programs.
void MakePixels(Random& random, std::uint8_t pixels[64])
{
    const std::uint32_t kind = random.Next() % 8;
    auto byte = [&random] { return static_cast<std::uint8_t>(random.Next()); };
    auto clamp = [](int v) { return static_cast<std::uint8_t>(v < 0 ? 0 : (v > 255 ? 255 : v)); };
    switch (kind)
    {
    case 0: // noise
        for (int i = 0; i < 64; ++i)
            pixels[i] = byte();
        break;
    case 1: // from one colour to another, with a little noise
    {
        std::uint8_t a[4], b[4];
        for (int c = 0; c < 4; ++c)
            a[c] = byte(), b[c] = byte();
        const std::uint32_t noise = random.Next() % 9;
        for (int i = 0; i < 16; ++i)
        {
            const int t = static_cast<int>(random.Next() % 256);
            for (int c = 0; c < 4; ++c)
            {
                const int n = noise ? static_cast<int>(random.Next() % (noise + 1)) - static_cast<int>(noise / 2) : 0;
                pixels[i * 4 + c] = clamp((a[c] * (255 - t) + b[c] * t) / 255 + n);
            }
        }
        break;
    }
    case 2: // greys
        for (int i = 0; i < 16; ++i)
        {
            const std::uint8_t v = byte();
            pixels[i * 4] = pixels[i * 4 + 1] = pixels[i * 4 + 2] = v;
            pixels[i * 4 + 3] = byte();
        }
        break;
    case 3: // greys one step apart
    {
        const std::uint32_t base = random.Next() % 255;
        for (int i = 0; i < 16; ++i)
        {
            const std::uint8_t v = static_cast<std::uint8_t>(base + random.Next() % 2);
            pixels[i * 4] = pixels[i * 4 + 1] = pixels[i * 4 + 2] = v;
            pixels[i * 4 + 3] = static_cast<std::uint8_t>(base + random.Next() % 3);
        }
        break;
    }
    case 4: // one colour
    case 5: // one colour but for a pixel
    {
        std::uint8_t a[4];
        for (int c = 0; c < 4; ++c)
            a[c] = byte();
        for (int i = 0; i < 16; ++i)
            for (int c = 0; c < 4; ++c)
                pixels[i * 4 + c] = a[c];
        if (kind == 5)
        {
            const std::uint32_t at = random.Next() % 16;
            for (int c = 0; c < 4; ++c)
                pixels[at * 4 + c] = static_cast<std::uint8_t>(a[c] + 1 + random.Next() % 40);
        }
        break;
    }
    case 6: // dark, with pixels that count as black
        for (int i = 0; i < 16; ++i)
        {
            const bool bright = random.Next() % 4 == 0;
            for (int c = 0; c < 4; ++c)
                pixels[i * 4 + c] = static_cast<std::uint8_t>(bright ? random.Next() : random.Next() % 8);
        }
        break;
    default: // three colours
    {
        std::uint8_t palette[3][4];
        for (int k = 0; k < 3; ++k)
            for (int c = 0; c < 4; ++c)
                palette[k][c] = byte();
        for (int i = 0; i < 16; ++i)
        {
            const std::uint32_t k = random.Next() % 3;
            for (int c = 0; c < 4; ++c)
                pixels[i * 4 + c] = palette[k][c];
        }
        break;
    }
    }
}

const rgbcx::bc1_approx_mode kModes[4] = {rgbcx::bc1_approx_mode::cBC1Ideal, rgbcx::bc1_approx_mode::cBC1NVidia, rgbcx::bc1_approx_mode::cBC1AMD,
    rgbcx::bc1_approx_mode::cBC1IdealRound4};

void DumpUnpack(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    std::uint32_t punchthrough = 0;
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        std::uint8_t block[16];
        for (int k = 0; k < 16; ++k)
            block[k] = static_cast<std::uint8_t>(random.Next());
        const rgbcx::bc1_approx_mode mode = kModes[i % 4];
        std::uint8_t pixels[64];

        rgbcx::color32 colors[4];
        std::uint8_t flag = rgbcx::unpack_bc1_block_colors(block, colors, mode) ? 1 : 0;
        hash.Add(&flag, 1);
        hash.Add(colors, sizeof colors);

        std::memset(pixels, 0x77, sizeof pixels);
        flag = rgbcx::unpack_bc1(block, pixels, true, mode) ? 1 : 0;
        punchthrough += flag;
        hash.Add(&flag, 1);
        hash.Add(pixels, sizeof pixels);

        std::memset(pixels, 0x77, sizeof pixels);
        flag = rgbcx::unpack_bc1(block + 8, pixels, false, mode) ? 1 : 0;
        hash.Add(&flag, 1);
        hash.Add(pixels, sizeof pixels);

        std::memset(pixels, 0x77, sizeof pixels);
        flag = rgbcx::unpack_bc3(block, pixels, mode) ? 1 : 0;
        hash.Add(&flag, 1);
        hash.Add(pixels, sizeof pixels);

        std::memset(pixels, 0x77, sizeof pixels);
        rgbcx::unpack_bc4(block, pixels, 1);
        rgbcx::unpack_bc4(block + 8, pixels + 16, 3);
        hash.Add(pixels, sizeof pixels);

        std::memset(pixels, 0x77, sizeof pixels);
        rgbcx::unpack_bc5(block, pixels, 0, 1, 4);
        rgbcx::unpack_bc5(block, pixels, 3, 2, 4);
        hash.Add(pixels, sizeof pixels);

        // A solid block of a made-up colour, and of one whose channels are close together.
        if (i % 4 == 0)
            rgbcx::init(kModes[(i / 4) % 4]);
        std::uint8_t solid[8];
        std::uint32_t r = random.Next() % 256, g = random.Next() % 256, b = random.Next() % 256;
        rgbcx::encode_bc1_solid_block(solid, r, g, b, i % 2 == 0);
        hash.Add(solid, sizeof solid);
        g = (r + random.Next() % 3) % 256, b = (r + random.Next() % 3) % 256;
        rgbcx::encode_bc1_solid_block(solid, r, g, b, i % 2 == 1);
        hash.Add(solid, sizeof solid);

        if ((i + 1) % 100000 == 0 || i + 1 == blocks)
            std::printf("unpack %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    std::printf("unpack punchthrough: %u\n", punchthrough);
}

void DumpBc1(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    std::uint32_t three_color = 0;
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        // The four ways of interpolating take turns, a hundred blocks each.
        const rgbcx::bc1_approx_mode mode = kModes[(i / 100) % 4];
        if (i % 100 == 0)
            rgbcx::init(mode);

        std::uint8_t pixels[64];
        MakePixels(random, pixels);

        std::uint8_t forced[16];
        for (int k = 0; k < 16; ++k)
            forced[k] = static_cast<std::uint8_t>(random.Next() % 4);
        const std::uint32_t options = random.Next();
        const std::uint8_t* force = (options & 15) == 0 ? forced : nullptr;

        std::uint8_t block[8];
        std::memset(block, 0xEE, sizeof block);
        if (i % 3 != 2)
        {
            // By level: every level in turn, the slowest (19) once in 64 of its turns.
            std::uint32_t level = i % 20;
            if (level == 19 && (i / 20) % 64 != 0)
                level = 5 + (i / 20) % 3;
            rgbcx::encode_bc1(level, block, pixels, (options & 16) != 0, (options & 32) != 0, force);
        }
        else
        {
            // By flags: any of them, with made-up numbers of orderings and rounds.
            static const std::uint32_t kFlags[13] = {rgbcx::cEncodeBC1UseLikelyTotalOrderings, rgbcx::cEncodeBC1TwoLeastSquaresPasses,
                rgbcx::cEncodeBC1Use3ColorBlocksForBlackPixels, rgbcx::cEncodeBC1Use3ColorBlocks, rgbcx::cEncodeBC1Iterative, rgbcx::cEncodeBC1BoundingBox,
                rgbcx::cEncodeBC1UseFasterMSEEval, rgbcx::cEncodeBC1UseFullMSEEval, rgbcx::cEncodeBC1Use2DLS, rgbcx::cEncodeBC1Use6PowerIters,
                rgbcx::cEncodeBC1TryAllInitialEndponts, rgbcx::cEncodeBC1BoundingBoxInt, 0};
            std::uint32_t flags = 0;
            const std::uint32_t bits = random.Next();
            for (int k = 0; k < 13; ++k)
                if (bits & (1u << k))
                    flags |= kFlags[k];
            // (Without a need for the block's error the encoder takes one first guess only.)
            const bool needs_error = (flags & (rgbcx::cEncodeBC1UseLikelyTotalOrderings | rgbcx::cEncodeBC1Use3ColorBlocks | rgbcx::cEncodeBC1UseFullMSEEval)) != 0;
            if ((bits & (1u << 13)) && needs_error)
                flags |= (random.Next() % 40) << rgbcx::cEncodeBC1EndpointSearchRoundsShift;
            if (!needs_error && !(flags & rgbcx::cEncodeBC1EndpointSearchRoundsMask))
                flags &= ~static_cast<std::uint32_t>(rgbcx::cEncodeBC1TryAllInitialEndponts);
            const std::uint32_t orderings4 = random.Next() % 140, orderings3 = random.Next() % 40;
            rgbcx::encode_bc1(block, pixels, flags, orderings4, orderings3, force);
        }
        hash.Add(block, sizeof block);

        std::uint8_t unpacked[64];
        const std::uint8_t flag = rgbcx::unpack_bc1(block, unpacked, true, mode) ? 1 : 0;
        three_color += flag;
        hash.Add(&flag, 1);
        hash.Add(unpacked, sizeof unpacked);

        if ((i + 1) % 20000 == 0 || i + 1 == blocks)
            std::printf("bc1 %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    std::printf("bc1 blocks of three colours: %u\n", three_color);
}

void DumpBc345(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    std::uint64_t error = 0;
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        const rgbcx::bc1_approx_mode mode = kModes[(i / 100) % 4];
        if (i % 100 == 0)
            rgbcx::init(mode);

        std::uint8_t pixels[64];
        MakePixels(random, pixels);
        const std::uint32_t options = random.Next();
        const std::uint32_t level = options % 13;
        const std::uint32_t radius = (options >> 8) % 6;
        const std::uint32_t modes = 1 + (options >> 12) % 3;
        std::uint8_t forced[16];
        for (int k = 0; k < 16; ++k)
            forced[k] = static_cast<std::uint8_t>(random.Next() % 8);

        std::uint8_t block[16];
        std::uint8_t unpacked[64];

        std::memset(block, 0xEE, sizeof block);
        rgbcx::encode_bc3(level, block, pixels);
        hash.Add(block, sizeof block);
        std::uint8_t flag = rgbcx::unpack_bc3(block, unpacked, mode) ? 1 : 0;
        hash.Add(&flag, 1);
        hash.Add(unpacked, sizeof unpacked);

        std::memset(block, 0xEE, sizeof block);
        // (Without a need for the block's error the encoder takes one first guess only.)
        std::uint32_t flags = options >> 16;
        if (!(flags & (rgbcx::cEncodeBC1UseLikelyTotalOrderings | rgbcx::cEncodeBC1UseFullMSEEval)))
            flags &= ~static_cast<std::uint32_t>(rgbcx::cEncodeBC1TryAllInitialEndponts);
        rgbcx::encode_bc3(block, pixels, flags, 1 + (options >> 4) % 12);
        hash.Add(block, sizeof block);

        std::memset(block, 0xEE, sizeof block);
        rgbcx::encode_bc3_hq(level, block, pixels, radius, modes);
        hash.Add(block, sizeof block);

        std::memset(block, 0xEE, sizeof block);
        rgbcx::encode_bc4(block, pixels + i % 4, 4);
        error += rgbcx::encode_bc4_hq(block + 8, pixels + i % 4, 4, radius, modes, (options & 0x80) ? forced : nullptr);
        hash.Add(block, sizeof block);
        rgbcx::unpack_bc4(block, unpacked, 1);
        rgbcx::unpack_bc4(block + 8, unpacked + 16, 1);
        hash.Add(unpacked, 32);

        std::memset(block, 0xEE, sizeof block);
        rgbcx::encode_bc5(block, pixels, 0, 1, 4);
        hash.Add(block, sizeof block);
        rgbcx::encode_bc5(block, pixels, 2, 3, 4);
        hash.Add(block, sizeof block);
        rgbcx::encode_bc5_hq(block, pixels, 1, 2, 4, radius, modes);
        hash.Add(block, sizeof block);
        std::memset(unpacked, 0x77, sizeof unpacked);
        rgbcx::unpack_bc5(block, unpacked, 1, 2, 4);
        hash.Add(unpacked, sizeof unpacked);

        if ((i + 1) % 20000 == 0 || i + 1 == blocks)
            std::printf("bc345 %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    std::printf("bc345 error of the searched BC4 blocks: %llu\n", static_cast<unsigned long long>(error));
}

std::uint32_t Bc7Mode(const std::uint8_t block[16])
{
    std::uint32_t mode = 0;
    while (mode < 8 && !(block[0] & (1u << mode)))
        ++mode;
    return mode;
}

void DumpBc7Enc(std::uint64_t seed, std::uint32_t blocks)
{
    static const std::uint32_t kPartitions[8] = {64, 64, 64, 32, 16, 1, 0, 100};
    static const std::uint32_t kMasks[8] = {UINT32_MAX, UINT32_MAX, UINT32_MAX, UINT32_MAX, 1u << 6, (1u << 1) | (1u << 5), (1u << 1) | (1u << 7), (1u << 1) | (1u << 5) | (1u << 7)};
    static const float kPbitWeights[4] = {1.0f, 1.0f, 1.3f, 0.75f};
    static const float kErrorWeights[4] = {1.0f, 1.0f, 1.5f, 0.5f};
    static const float kPartitionWeights[4] = {1.0f, 1.0f, 0.8f, 1.25f};

    Random random{seed};
    Hash hash;
    bc7enc_compress_block_init();
    std::uint32_t packed[9] = {};
    std::uint32_t with_alpha = 0;
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        std::uint8_t pixels[64];
        MakePixels(random, pixels);
        const std::uint32_t o1 = random.Next();
        const std::uint32_t o2 = random.Next();
        std::uint32_t weights[4];
        for (int k = 0; k < 4; ++k)
            weights[k] = 1 + random.Next() % 255;
        std::uint8_t selectors[16];
        for (int k = 0; k < 16; ++k)
            selectors[k] = static_cast<std::uint8_t>(random.Next());

        bool opaque = o1 % 2 == 0;

        bc7enc_compress_block_params params;
        bc7enc_compress_block_params_init(&params);
        std::memset(params.m_selectors, 0, sizeof params.m_selectors);
        params.m_uber_level = (o1 >> 4) % 5;
        if (!((o1 >> 8) & 1))
            bc7enc_compress_block_params_init_linear_weights(&params);
        if ((o1 >> 9) % 4 == 0)
            std::memcpy(params.m_weights, weights, sizeof weights);
        params.m_try_least_squares = (o1 >> 11) % 4 != 0;
        params.m_mode17_partition_estimation_filterbank = ((o1 >> 13) & 1) != 0;
        params.m_max_partitions = kPartitions[(o1 >> 14) % 8];
        params.m_force_alpha = (o1 >> 17) % 8 == 0;
        params.m_quant_mode6_endpoints = (o1 >> 20) % 4 == 0;
        params.m_bias_mode1_pbits = (o1 >> 22) % 4 == 0;
        params.m_mode_mask = kMasks[(o1 >> 24) % 8];
        params.m_pbit1_weight = kPbitWeights[(o2 >> 8) % 4];
        params.m_mode1_error_weight = kErrorWeights[(o2 >> 10) % 4];
        params.m_mode5_error_weight = kErrorWeights[(o2 >> 12) % 4];
        params.m_mode6_error_weight = kErrorWeights[(o2 >> 14) % 4];
        params.m_mode7_error_weight = kErrorWeights[(o2 >> 16) % 4];
        params.m_low_frequency_partition_weight = kPartitionWeights[(o2 >> 18) % 4];
        if (o2 % 8 == 0)
        {
            // The selectors have to fit the modes they may be used with.
            params.m_force_selectors = true;
            const std::uint32_t how = (o2 >> 3) % 3;
            if (how == 1)
                params.m_mode_mask = 1u << 6;
            else if (how == 2)
            {
                params.m_mode_mask = 1u << 1;
                params.m_force_alpha = false;
                opaque = true;
            }
            for (int k = 0; k < 16; ++k)
                params.m_selectors[k] = selectors[k] % (how == 0 ? 4 : (how == 1 ? 16 : 8));
        }
        // (Mode 1 alone needs a partition to try.)
        if (!(params.m_mode_mask & (1u << 6)) && params.m_max_partitions == 0)
            params.m_max_partitions = 64;
        if (opaque)
            for (int k = 0; k < 16; ++k)
                pixels[k * 4 + 3] = 255;

        std::uint8_t block[16];
        std::memset(block, 0xEE, sizeof block);
        const std::uint8_t flag = bc7enc_compress_block(block, pixels, &params) ? 1 : 0;
        with_alpha += flag;
        hash.Add(block, sizeof block);
        hash.Add(&flag, 1);
        packed[Bc7Mode(block)] += 1;

        bc7decomp::color_rgba unpacked[16];
        std::memset(unpacked, 0xEE, sizeof unpacked);
        const std::uint8_t ok = bc7decomp::unpack_bc7(block, unpacked) ? 1 : 0;
        hash.Add(&ok, 1);
        hash.Add(unpacked, sizeof unpacked);

        if ((i + 1) % 20000 == 0 || i + 1 == blocks)
            std::printf("bc7enc %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    std::printf("bc7enc blocks with alpha: %u, packed by mode:", with_alpha);
    for (std::uint32_t mode = 0; mode < 9; ++mode)
        std::printf(" %u", packed[mode]);
    std::printf("\n");
}

void DumpBc7Block(std::uint64_t seed, std::uint32_t blocks)
{
    static const std::uint32_t kPartitionBits[8] = {4, 6, 6, 6, 0, 0, 0, 6};
    static const std::uint32_t kColorIndexBits[8] = {3, 3, 2, 2, 2, 2, 4, 2};
    static const std::uint32_t kAlphaIndexBits[8] = {0, 0, 0, 0, 3, 2, 4, 2};
    static const std::uint32_t kColorBits[8] = {4, 6, 5, 7, 5, 7, 7, 5};
    static const std::uint32_t kAlphaBits[8] = {0, 0, 0, 0, 6, 8, 7, 5};

    Random random{seed};
    Hash hash;
    std::uint32_t unpacked_ok = 0;
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        bc7_optimization_results results;
        std::memset(&results, 0, sizeof results);
        const std::uint32_t mode = i % 8;
        results.m_mode = mode;
        results.m_partition = random.Next() % (1u << kPartitionBits[mode]);
        results.m_rotation = (mode == 4 || mode == 5) ? random.Next() % 4 : 0;
        results.m_index_selector = mode == 4 ? random.Next() % 2 : 0;
        const std::uint32_t color_index_bits = kColorIndexBits[mode] + results.m_index_selector;
        const std::uint32_t alpha_index_bits = kAlphaIndexBits[mode] - results.m_index_selector;
        for (int k = 0; k < 16; ++k)
        {
            results.m_selectors[k] = static_cast<std::uint8_t>(random.Next() % (1u << color_index_bits));
            results.m_alpha_selectors[k] = static_cast<std::uint8_t>((mode == 4 || mode == 5) ? random.Next() % (1u << alpha_index_bits) : 0);
        }
        for (int k = 0; k < 3; ++k)
        {
            for (int c = 0; c < 4; ++c)
            {
                const std::uint32_t bits = c == 3 ? kAlphaBits[mode] : kColorBits[mode];
                results.m_low[k].m_c[c] = static_cast<std::uint8_t>(random.Next() % (1u << bits));
                results.m_high[k].m_c[c] = static_cast<std::uint8_t>(random.Next() % (1u << bits));
            }
            results.m_pbits[k][0] = random.Next() % 2;
            results.m_pbits[k][1] = random.Next() % 2;
        }

        std::uint8_t block[16];
        std::memset(block, 0xEE, sizeof block);
        encode_bc7_block(block, &results);
        hash.Add(block, sizeof block);

        bc7decomp::color_rgba unpacked[16];
        std::memset(unpacked, 0xEE, sizeof unpacked);
        const std::uint8_t ok = bc7decomp::unpack_bc7(block, unpacked) ? 1 : 0;
        unpacked_ok += ok;
        hash.Add(&ok, 1);
        hash.Add(unpacked, sizeof unpacked);

        if ((i + 1) % 100000 == 0 || i + 1 == blocks)
            std::printf("bc7block %u %016llx\n", i + 1, static_cast<unsigned long long>(hash.value));
    }
    std::printf("bc7block unpacked: %u\n", unpacked_ok);
}
// The unpackers the entropy reduction transform is given.
bool ErtUnpackBc1(const void* block, ert::color_rgba* pixels, std::uint32_t, void*)
{
    rgbcx::unpack_bc1(block, pixels, true, rgbcx::bc1_approx_mode::cBC1Ideal);
    return true;
}
bool ErtUnpackBc4(const void* block, ert::color_rgba* pixels, std::uint32_t, void*)
{
    rgbcx::unpack_bc4(block, &pixels[0].m_c[0], 4);
    return true;
}
bool ErtUnpackBc5(const void* block, ert::color_rgba* pixels, std::uint32_t, void*)
{
    rgbcx::unpack_bc5(block, pixels, 0, 1, 4);
    return true;
}
bool ErtUnpackBc7(const void* block, ert::color_rgba* pixels, std::uint32_t, void*)
{
    return bc7decomp::unpack_bc7(block, reinterpret_cast<bc7decomp::color_rgba*>(pixels));
}

// Runs of made-up blocks, many of them near copies of the blocks before, packed in one of five
// formats and put through the entropy reduction transform with its settings varied.
void DumpErt(std::uint64_t seed, std::uint32_t blocks)
{
    Random random{seed};
    Hash hash;
    rgbcx::init(rgbcx::bc1_approx_mode::cBC1Ideal);
    bc7enc_compress_block_init();
    bc7enc_compress_block_params bc7_params;
    bc7enc_compress_block_params_init(&bc7_params);
    std::uint32_t remaining = blocks, runs = 0, modified = 0, failed = 0;
    std::vector<std::uint8_t> px, packed;
    std::vector<ert::color_rgba> alpha;
    std::vector<float> scales;
    while (remaining > 0)
    {
        const std::uint32_t format = random.Next() % 5;
        const bool relative = random.Next() % 8 == 0;
        std::uint32_t n = relative ? 8 + random.Next() % 57 : 32 + random.Next() % 481;
        if (n > remaining)
            n = remaining;
        remaining -= n;

        px.assign(static_cast<std::size_t>(n) * 64, 0);
        for (std::uint32_t i = 0; i < n; ++i)
        {
            std::uint8_t* block = &px[static_cast<std::size_t>(i) * 64];
            const std::uint32_t r = random.Next() % 8;
            if (i > 0 && r < 3)
                std::memcpy(block, block - 64, 64);
            else if (i > 3 && r == 3)
                std::memcpy(block, block - 64 * (1 + random.Next() % 3), 64);
            else
                MakePixels(random, block);
            if (i > 0 && r < 2)
            {
                const std::uint32_t at = random.Next() % 64;
                block[at] = static_cast<std::uint8_t>(block[at] + 1);
            }
        }

        ert::reduce_entropy_params p;
        p.m_lambda = static_cast<float>(random.Next() % 2000) / 100.0f + 0.01f;
        static const std::uint32_t kWindows[6] = {16, 48, 128, 256, 1024, 4096};
        p.m_lookback_window_size = kWindows[random.Next() % 6];
        p.m_max_allowed_rms_increase_ratio = 1.0f + static_cast<float>(random.Next() % 1000) / 100.0f;
        p.m_max_smooth_block_std_dev = 1.0f + static_cast<float>(random.Next() % 40);
        p.m_smooth_block_max_mse_scale = 1.0f + static_cast<float>(random.Next() % 20);
        p.m_try_two_matches = random.Next() % 2 != 0;
        p.m_allow_relative_movement = relative;
        p.m_skip_zero_mse_blocks = random.Next() % 4 == 0;
        std::uint32_t weights[4] = {1, 1, 1, 1};
        if (random.Next() % 2 != 0)
            for (int c = 0; c < 4; ++c)
                weights[c] = 1 + random.Next() % 4;
        const bool use_scales = random.Next() % 3 == 0;
        scales.assign(n, 0.0f);
        if (use_scales)
            for (std::uint32_t i = 0; i < n; ++i)
            {
                const std::uint32_t a = random.Next();
                scales[i] = a % 4 == 0 ? 0.0f : 1.0f + static_cast<float>((a >> 8) % 10);
            }
        std::vector<float>* scale_list = use_scales ? &scales : nullptr;
        const ert::color_rgba* pixels = reinterpret_cast<const ert::color_rgba*>(px.data());

        const std::uint32_t stride = format < 2 ? 8 : 16;
        packed.assign(static_cast<std::size_t>(n) * stride, 0);
        const std::uint32_t level = random.Next() % 6;
        std::uint32_t total = 0;
        bool ok = true;
        auto set_weights = [&p, &weights](std::uint32_t comps) {
            for (std::uint32_t c = 0; c < 4; ++c)
                p.m_color_weights[c] = c < comps ? weights[c] : 0;
        };
        switch (format)
        {
        case 0:
            for (std::uint32_t i = 0; i < n; ++i)
                rgbcx::encode_bc1(level, &packed[static_cast<std::size_t>(i) * 8], &px[static_cast<std::size_t>(i) * 64], false, false);
            set_weights(3);
            ok = ert::reduce_entropy(packed.data(), n, 8, 8, 4, 4, 3, pixels, p, total, ErtUnpackBc1, nullptr, scale_list);
            break;
        case 1:
            for (std::uint32_t i = 0; i < n; ++i)
                rgbcx::encode_bc4(&packed[static_cast<std::size_t>(i) * 8], &px[static_cast<std::size_t>(i) * 64], 4);
            set_weights(1);
            ok = ert::reduce_entropy(packed.data(), n, 8, 8, 4, 4, 1, pixels, p, total, ErtUnpackBc4, nullptr, scale_list);
            break;
        case 2:
            for (std::uint32_t i = 0; i < n; ++i)
                bc7enc_compress_block(&packed[static_cast<std::size_t>(i) * 16], &px[static_cast<std::size_t>(i) * 64], &bc7_params);
            set_weights(4);
            ok = ert::reduce_entropy(packed.data(), n, 16, 16, 4, 4, 4, pixels, p, total, ErtUnpackBc7, nullptr, scale_list);
            break;
        case 3:
            for (std::uint32_t i = 0; i < n; ++i)
                rgbcx::encode_bc3(level, &packed[static_cast<std::size_t>(i) * 16], &px[static_cast<std::size_t>(i) * 64]);
            // The alpha half, against the alpha of the pixels put in the first channel.
            alpha.assign(static_cast<std::size_t>(n) * 16, ert::color_rgba{{0, 0, 0, 0}});
            for (std::size_t i = 0; i < alpha.size(); ++i)
                alpha[i].m_c[0] = px[i * 4 + 3];
            set_weights(1);
            ok = ert::reduce_entropy(packed.data(), n, 16, 8, 4, 4, 1, alpha.data(), p, total, ErtUnpackBc4, nullptr, scale_list);
            // The colour half.
            set_weights(3);
            ok = ert::reduce_entropy(packed.data() + 8, n, 16, 8, 4, 4, 3, pixels, p, total, ErtUnpackBc1, nullptr, scale_list) && ok;
            break;
        default:
            for (std::uint32_t i = 0; i < n; ++i)
                rgbcx::encode_bc5(&packed[static_cast<std::size_t>(i) * 16], &px[static_cast<std::size_t>(i) * 64], 0, 1, 4);
            set_weights(2);
            ok = ert::reduce_entropy(packed.data(), n, 16, 16, 4, 4, 2, pixels, p, total, ErtUnpackBc5, nullptr, scale_list);
            break;
        }
        hash.Add(packed.data(), packed.size());
        modified += total;
        failed += ok ? 0 : 1;
        ++runs;
    }
    std::printf("ert %016llx\n", static_cast<unsigned long long>(hash.value));
    std::printf("ert runs: %u, blocks changed: %u, runs refused: %u\n", runs, modified, failed);
}
// A made-up image: noise, a ramp, flat boxes, a repeated tile, a very smooth field, or half a
// ramp and half noise; opaque, with noise for alpha, or with a ramp of alpha.
utils::image_u8 MakeImage(Random& random, std::uint32_t w, std::uint32_t h)
{
    utils::image_u8 image(w, h);
    const std::uint32_t kind = random.Next() % 6;
    const std::uint32_t alpha_kind = random.Next() % 3;
    auto byte = [&random] { return static_cast<std::uint8_t>(random.Next()); };
    auto clamp = [](int v) { return static_cast<std::uint8_t>(v < 0 ? 0 : (v > 255 ? 255 : v)); };
    std::uint8_t a[4], b[4];
    for (int c = 0; c < 4; ++c)
        a[c] = byte(), b[c] = byte();
    const std::uint32_t noise = random.Next() % 4;
    std::uint8_t tile[256];
    for (int i = 0; i < 256; ++i)
        tile[i] = byte();
    for (std::uint32_t y = 0; y < h; ++y)
        for (std::uint32_t x = 0; x < w; ++x)
        {
            std::uint8_t p[4];
            switch (kind)
            {
            case 0:
                for (int c = 0; c < 4; ++c)
                    p[c] = byte();
                break;
            case 1:
            {
                const int t = static_cast<int>((x * 255) / (w > 1 ? w - 1 : 1));
                for (int c = 0; c < 4; ++c)
                {
                    const int n = noise ? static_cast<int>(random.Next() % (noise + 1)) : 0;
                    p[c] = clamp((a[c] * (255 - t) + b[c] * t) / 255 + n);
                }
                break;
            }
            case 2:
            {
                const bool in_box = ((x / 16) + (y / 12)) % 3 == 0;
                for (int c = 0; c < 4; ++c)
                    p[c] = in_box ? b[c] : a[c];
                break;
            }
            case 3:
                for (int c = 0; c < 4; ++c)
                    p[c] = tile[((x % 8) + (y % 8) * 8) * 4 + c];
                break;
            case 4:
                for (int c = 0; c < 4; ++c)
                    p[c] = static_cast<std::uint8_t>(a[c] / 2 + 60 + random.Next() % 2);
                break;
            default:
                if (x < w / 2)
                {
                    const int t = static_cast<int>((y * 255) / (h > 1 ? h - 1 : 1));
                    for (int c = 0; c < 4; ++c)
                        p[c] = static_cast<std::uint8_t>((a[c] * (255 - t) + b[c] * t) / 255);
                }
                else
                    for (int c = 0; c < 4; ++c)
                        p[c] = byte();
                break;
            }
            if (alpha_kind == 0)
                p[3] = 255;
            else if (alpha_kind == 2)
                p[3] = static_cast<std::uint8_t>((x + y) * 3);
            image(x, y) = utils::color_quad_u8(p[0], p[1], p[2], p[3]);
        }
    return image;
}

// Made-up images packed whole by rdo_bc_encoder.
void DumpRdo(std::uint64_t seed, std::uint32_t blocks, bool with_bc7e)
{
    Random random{seed};
    Hash hash;
    std::uint32_t done = 0, images = 0, refused = 0;
    while (done < blocks)
    {
        const bool big = random.Next() % 6 == 0;
        const std::uint32_t w = big ? 240 + random.Next() % 100 : 1 + random.Next() % 96;
        const std::uint32_t h = big ? 240 + random.Next() % 100 : 1 + random.Next() % 96;
        const utils::image_u8 image = MakeImage(random, w, h);

        rdo_bc::rdo_bc_params p;
        static const DXGI_FORMAT kFormats[5] = {DXGI_FORMAT_BC1_UNORM, DXGI_FORMAT_BC3_UNORM, DXGI_FORMAT_BC4_UNORM, DXGI_FORMAT_BC5_UNORM, DXGI_FORMAT_BC7_UNORM};
        p.m_dxgi_format = kFormats[random.Next() % 5];
        const std::uint32_t level = random.Next();
        p.m_bc1_quality_level = static_cast<int>(big ? level % 3 : level % 11);
        p.m_use_bc1_3color_mode = random.Next() % 2 != 0;
        p.m_use_bc1_3color_mode_for_black = random.Next() % 2 != 0;
        p.m_bc1_mode = kModes[random.Next() % 4];
        p.m_y_flip = random.Next() % 4 == 0;
        p.m_bc45_channel0 = random.Next() % 4;
        p.m_bc45_channel1 = random.Next() % 4;
        p.m_use_hq_bc345 = random.Next() % 2 != 0;
        p.m_bc345_search_rad = static_cast<int>(random.Next() % 8);
        p.m_bc345_mode_mask = 1 + random.Next() % 3;
        const bool use_bc7e = random.Next() % 2 != 0;
        p.m_use_bc7e = with_bc7e && use_bc7e;
        const std::uint32_t uber = random.Next();
        p.m_bc7_uber_level = static_cast<int>(big ? uber % 2 : (w * h <= 256 ? uber % 7 : uber % 5));
        p.m_bc7enc_max_partitions_to_scan = static_cast<int>(random.Next() % 65);
        p.m_perceptual = random.Next() % 2 != 0;
        p.m_bc7enc_mode6_only = random.Next() % 8 == 0;
        p.m_bc7enc_reduce_entropy = random.Next() % 2 != 0;
        p.m_bc7enc_rdo_bc7_quant_mode6_endpoints = random.Next() % 2 != 0;
        p.m_bc7enc_rdo_bc7_weight_modes = random.Next() % 2 != 0;
        p.m_bc7enc_rdo_bc7_weight_low_frequency_partitions = random.Next() % 2 != 0;
        p.m_bc7enc_rdo_bc7_pbit1_weighting = random.Next() % 2 != 0;
        const bool lambda_on = random.Next() % 3 != 0;
        const std::uint32_t lambda = random.Next();
        p.m_rdo_lambda = lambda_on ? static_cast<float>(lambda % 1000) / 100.0f + 0.05f : 0.0f;
        static const std::uint32_t kWindows[6] = {8, 16, 64, 128, 256, 1024};
        p.m_lookback_window_size = kWindows[random.Next() % 6];
        p.m_custom_rdo_smooth_block_error_scale = random.Next() % 2 != 0;
        p.m_rdo_smooth_block_error_scale = 1.0f + static_cast<float>(random.Next() % 40);
        p.m_rdo_max_smooth_block_std_dev = 1.0f + static_cast<float>(random.Next() % 40);
        const bool relative = random.Next() % 4 == 0;
        p.m_rdo_allow_relative_movement = relative && !big && w * h <= 1024;
        p.m_rdo_try_2_matches = random.Next() % 2 != 0;
        p.m_rdo_ultrasmooth_block_handling = random.Next() % 2 != 0;
        p.m_rdo_multithreading = random.Next() % 4 != 0;
        p.m_rdo_max_threads = 1 + random.Next() % 8;

        // (The encoder prints its settings for BC1 whether asked or not: not into the dump.)
        std::fflush(stdout);
        const int saved = dup(1), null = open("/dev/null", O_WRONLY);
        dup2(null, 1);
        rdo_bc::rdo_bc_encoder encoder;
        bool ok = encoder.init(image, p) && encoder.encode();
        utils::image_u8 unpacked;
        ok = ok && encoder.unpack_blocks(unpacked);
        std::fflush(stdout);
        dup2(saved, 1);
        close(saved);
        close(null);

        const std::uint32_t sizes[4] = {encoder.get_blocks_x(), encoder.get_blocks_y(), encoder.get_bytes_per_block(), ok ? 1u : 0u};
        hash.Add(sizes, sizeof(sizes));
        if (ok)
        {
            hash.Add(encoder.get_prerdo_blocks(), encoder.get_total_blocks_size_in_bytes());
            hash.Add(encoder.get_blocks(), encoder.get_total_blocks_size_in_bytes());
            hash.Add(unpacked.get_pixels().data(), unpacked.get_pixels().size() * 4);
        }
        else
            ++refused;
        done += encoder.get_total_blocks();
        ++images;
    }
    std::printf("rdo %016llx\n", static_cast<unsigned long long>(hash.value));
    std::printf("rdo images: %u, blocks: %u, refused: %u\n", images, done, refused);
}
} // namespace

int main(int argc, char** argv)
{
    if (argc < 4)
    {
        std::fprintf(stderr, "usage: dump <what> <seed> <blocks>   (see the top of dump.cpp)\n");
        return 2;
    }
    const std::uint64_t seed = std::strtoull(argv[2], nullptr, 10);
    const std::uint32_t blocks = static_cast<std::uint32_t>(std::strtoul(argv[3], nullptr, 10));
    if (std::strcmp(argv[1], "bc7decomp") == 0)
        DumpBc7Decomp(seed, blocks);
    else if (std::strcmp(argv[1], "unpack") == 0)
        DumpUnpack(seed, blocks);
    else if (std::strcmp(argv[1], "bc1") == 0)
        DumpBc1(seed, blocks);
    else if (std::strcmp(argv[1], "bc345") == 0)
        DumpBc345(seed, blocks);
    else if (std::strcmp(argv[1], "bc7enc") == 0)
        DumpBc7Enc(seed, blocks);
    else if (std::strcmp(argv[1], "bc7block") == 0)
        DumpBc7Block(seed, blocks);
    else if (std::strcmp(argv[1], "ert") == 0)
        DumpErt(seed, blocks);
    else if (std::strcmp(argv[1], "rdo") == 0)
        DumpRdo(seed, blocks, false);
#if SUPPORT_BC7E
    else if (std::strcmp(argv[1], "rdo_bc7e") == 0)
        DumpRdo(seed, blocks, true);
#endif
    else
        return 2;
    return 0;
}
