// The reference half of the comparison with bc7e.ispc: packs made-up blocks with the ISPC
// encoder and prints a hash of the blocks for each group. examples/dump.rs prints the same
// from the Rust port, and compare/compare.sh compares the two.
//
//   dump bc7e_presets <seed> <blocks> [file]    the same blocks with each of the seven
//                                               profiles, for both error measures
//   dump bc7e_settings <seed> <blocks> [file]   the blocks in groups of 32, each group with
//                                               settings made up from the seed
//
// With a file named, every packed block is written to it as well.

#include "bc7e_ispc.h"

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
        {
            a[c] = byte();
            b[c] = byte();
        }
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
    case 6: // dark, with a few bright pixels
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

// The blocks' pixels: half the blocks opaque.
std::vector<std::uint32_t> MakeBlocks(Random& random, std::uint32_t blocks)
{
    std::vector<std::uint32_t> pixels(static_cast<std::size_t>(blocks) * 16);
    for (std::uint32_t i = 0; i < blocks; ++i)
    {
        std::uint8_t p[64];
        MakePixels(random, p);
        if (random.Next() % 2 == 0)
            for (int k = 0; k < 16; ++k)
                p[k * 4 + 3] = 255;
        std::memcpy(&pixels[static_cast<std::size_t>(i) * 16], p, 64);
    }
    return pixels;
}

// Settings made up from the seed, kept to those that leave every block a mode to be packed in.
ispc::bc7e_compress_block_params MakeSettings(Random& random)
{
    static const std::uint32_t kPartitions[8] = {64, 64, 64, 32, 16, 1, 0, 100};
    static const std::uint32_t kPartitions0[4] = {16, 16, 8, 1};
    static const std::uint32_t kPasses[8] = {1, 1, 1, 1, 0, 2, 3, 1};
    static const std::uint32_t kToTry[8] = {1, 1, 2, 4, 8, 3, 64, 20};
    static const std::uint32_t kMul[4] = {1, 1, 2, 3};
    ispc::bc7e_compress_block_params p;
    ispc::bc7e_compress_block_params_init(&p, random.Next() % 2 != 0);
    p.m_max_partitions_mode[0] = kPartitions0[random.Next() % 4];
    p.m_max_partitions_mode[1] = kPartitions[random.Next() % 8];
    p.m_max_partitions_mode[2] = kPartitions[random.Next() % 8];
    p.m_max_partitions_mode[3] = kPartitions[random.Next() % 8];
    p.m_max_partitions_mode[7] = kPartitions[random.Next() % 8];
    const bool weights = random.Next() % 4 == 0;
    for (int c = 0; c < 4; ++c)
    {
        const std::uint32_t weight = 1 + random.Next() % 255;
        if (weights)
            p.m_weights[c] = weight;
    }
    p.m_uber_level = random.Next() % 5;
    p.m_refinement_passes = kPasses[random.Next() % 8];
    p.m_mode4_rotation_mask = 1 + random.Next() % 15;
    p.m_mode4_index_mask = 1 + random.Next() % 3;
    p.m_mode5_rotation_mask = 1 + random.Next() % 15;
    p.m_uber1_mask = 1 + random.Next() % 7;
    p.m_pbit_search = random.Next() % 2 != 0;
    p.m_mode6_only = random.Next() % 8 == 0;
    p.m_opaque_settings.m_max_mode13_partitions_to_try = kToTry[random.Next() % 8];
    p.m_opaque_settings.m_max_mode0_partitions_to_try = kToTry[random.Next() % 8];
    p.m_opaque_settings.m_max_mode2_partitions_to_try = kToTry[random.Next() % 8];
    for (int m = 0; m < 7; ++m)
        p.m_opaque_settings.m_use_mode[m] = random.Next() % 4 != 0;
    p.m_alpha_settings.m_max_mode7_partitions_to_try = kToTry[random.Next() % 8];
    for (int c = 0; c < 4; ++c)
        p.m_alpha_settings.m_mode67_error_weight_mul[c] = kMul[random.Next() % 4];
    p.m_alpha_settings.m_use_mode4 = random.Next() % 4 != 0;
    p.m_alpha_settings.m_use_mode5 = random.Next() % 4 != 0;
    p.m_alpha_settings.m_use_mode6 = random.Next() % 4 != 0;
    p.m_alpha_settings.m_use_mode7 = random.Next() % 4 != 0;
    p.m_alpha_settings.m_use_mode4_rotation = random.Next() % 2 != 0;
    p.m_alpha_settings.m_use_mode5_rotation = random.Next() % 2 != 0;

    const bool* use = p.m_opaque_settings.m_use_mode;
    if (!(use[0] || use[1] || use[2] || use[3] || use[6] || (!p.m_perceptual && (use[4] || use[5]))))
        p.m_opaque_settings.m_use_mode[6] = true;
    if (!(p.m_alpha_settings.m_use_mode6 || p.m_alpha_settings.m_use_mode7 || (p.m_alpha_settings.m_use_mode5 && (p.m_mode5_rotation_mask & 1)) ||
            (p.m_alpha_settings.m_use_mode4 && (p.m_mode4_rotation_mask & 1))))
        p.m_alpha_settings.m_use_mode6 = true;
    return p;
}

// FNV-1a, 64 bits, of the packed blocks; the blocks counted by mode.
std::uint64_t Hash(const std::vector<std::uint64_t>& blocks, std::uint32_t modes[9])
{
    std::uint64_t value = 14695981039346656037ull;
    const std::uint8_t* bytes = reinterpret_cast<const std::uint8_t*>(blocks.data());
    for (std::size_t i = 0; i < blocks.size() * 8; ++i)
    {
        value ^= bytes[i];
        value *= 1099511628211ull;
    }
    for (std::size_t i = 0; i < blocks.size() / 2; ++i)
    {
        std::uint32_t mode = 0;
        while (mode < 8 && !(bytes[i * 16] & (1u << mode)))
            ++mode;
        modes[mode] += 1;
    }
    return value;
}
} // namespace

int main(int argc, char** argv)
{
    if (argc < 4)
    {
        std::fprintf(stderr, "usage: dump bc7e_presets|bc7e_settings <seed> <blocks> [file]\n");
        return 2;
    }
    const bool presets = std::strcmp(argv[1], "bc7e_presets") == 0;
    if (!presets && std::strcmp(argv[1], "bc7e_settings") != 0)
        return 2;
    Random random{std::strtoull(argv[2], nullptr, 10)};
    const std::uint32_t blocks = static_cast<std::uint32_t>(std::strtoul(argv[3], nullptr, 10));
    std::FILE* file = argc > 4 ? std::fopen(argv[4], "wb") : nullptr;

    ispc::bc7e_compress_block_init();
    const std::vector<std::uint32_t> pixels = MakeBlocks(random, blocks);
    std::vector<std::uint64_t> packed(static_cast<std::size_t>(blocks) * 2);
    std::uint32_t modes[9] = {};

    if (presets)
    {
        static const char* const kNames[7] = {"ultrafast", "veryfast", "fast", "basic", "slow", "veryslow", "slowest"};
        for (int preset = 0; preset < 7; ++preset)
            for (int perceptual = 0; perceptual < 2; ++perceptual)
            {
                ispc::bc7e_compress_block_params params;
                switch (preset)
                {
                case 0: ispc::bc7e_compress_block_params_init_ultrafast(&params, perceptual != 0); break;
                case 1: ispc::bc7e_compress_block_params_init_veryfast(&params, perceptual != 0); break;
                case 2: ispc::bc7e_compress_block_params_init_fast(&params, perceptual != 0); break;
                case 3: ispc::bc7e_compress_block_params_init_basic(&params, perceptual != 0); break;
                case 4: ispc::bc7e_compress_block_params_init_slow(&params, perceptual != 0); break;
                case 5: ispc::bc7e_compress_block_params_init_veryslow(&params, perceptual != 0); break;
                default: ispc::bc7e_compress_block_params_init_slowest(&params, perceptual != 0); break;
                }
                ispc::bc7e_compress_blocks(blocks, packed.data(), pixels.data(), &params);
                std::printf("%s %s %016llx\n", kNames[preset], perceptual ? "perceptual" : "linear", static_cast<unsigned long long>(Hash(packed, modes)));
                if (file)
                    std::fwrite(packed.data(), 16, blocks, file);
            }
    }
    else
    {
        for (std::uint32_t first = 0; first < blocks; first += 32)
        {
            const std::uint32_t count = blocks - first < 32 ? blocks - first : 32;
            const ispc::bc7e_compress_block_params params = MakeSettings(random);
            ispc::bc7e_compress_blocks(count, &packed[static_cast<std::size_t>(first) * 2], &pixels[static_cast<std::size_t>(first) * 16], &params);
        }
        std::printf("settings %016llx\n", static_cast<unsigned long long>(Hash(packed, modes)));
        if (file)
            std::fwrite(packed.data(), 16, blocks, file);
    }
    std::printf("blocks by mode:");
    for (int mode = 0; mode < 9; ++mode)
        std::printf(" %u", modes[mode]);
    std::printf("\n");
    if (file)
        std::fclose(file);
    return 0;
}
