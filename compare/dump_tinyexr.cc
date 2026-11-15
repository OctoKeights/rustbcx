// The C++ half of the comparison with tinyexr (https://github.com/syoyo/tinyexr): makes .exr
// files of made-up pixels with it, reads them back with it, and prints a running hash of what
// it reads. examples/dump.rs prints the same from the Rust port, and compare/compare.sh
// compares the two.
//
//   dump tinyexr_read <seed> <count> <file>         made-up images of every kind tinyexr writes
//   dump tinyexr_read_small <seed> <count> <file>   (scanlines and tiles, with mip and rip
//                                                   levels, three types of value, eight
//                                                   compressions), saved; now and then the
//                                                   window moved or the order of the lines
//                                                   named the other way; a third of them
//                                                   damaged. Its version and its header are
//                                                   read, its image as it is stored or with
//                                                   halves as floats, and it as floats of red,
//                                                   green, blue and alpha; and it is written to
//                                                   the file, with how it was read.
//
// Built without threads and without OpenMP, against the miniz that src/miniz is the port of,
// and with memory that starts as zeros: tinyexr leaves what a damaged file does not fill as
// malloc gave it.
//
// tinyexr's PIZ writer gives a block 8192 bytes and twice the block's own, and writes past
// that for a small block of many different values (the bitmap alone can take 8192 bytes, and
// the table of code lengths comes after it). Everything allocated with new here is given
// 65536 bytes more than was asked for, so that what is written past the end stays in memory
// the program owns, and the block comes out as it would with room enough.
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <exception>
#include <limits>
#include <new>
#include <set>
#include <sstream>
#include <string>
#include <vector>

#define malloc(size) calloc(1, (size))
#define TINYEXR_USE_MINIZ 1
#define TINYEXR_USE_THREAD 0
#define TINYEXR_USE_OPENMP 0
#define TINYEXR_USE_PIZ 1
#define TINYEXR_USE_ZFP 0
#define TINYEXR_IMPLEMENTATION
#include "tinyexr.h"
#undef malloc

void* operator new(std::size_t size)
{
    void* memory = std::malloc(size + 65536);
    if (!memory)
        throw std::bad_alloc();
    return memory;
}
void operator delete(void* memory) noexcept
{
    std::free(memory);
}
void operator delete(void* memory, std::size_t) noexcept
{
    std::free(memory);
}

namespace
{
// The two programs make the same data from the same seed: a 64-bit linear congruential
// generator, the high half of its state.
std::uint64_t g_state;
std::uint32_t Next()
{
    g_state = g_state * 6364136223846793005ull + 1442695040888963407ull;
    return static_cast<std::uint32_t>(g_state >> 32);
}

// FNV-1a, 64 bits.
std::uint64_t g_hash = 14695981039346656037ull;
void Add(const void* data, std::size_t size)
{
    const auto* bytes = static_cast<const unsigned char*>(data);
    for (std::size_t i = 0; i < size; ++i)
    {
        g_hash ^= bytes[i];
        g_hash *= 1099511628211ull;
    }
}
void AddNumber(std::uint32_t value)
{
    unsigned char bytes[4];
    bytes[0] = static_cast<unsigned char>(value);
    bytes[1] = static_cast<unsigned char>(value >> 8);
    bytes[2] = static_cast<unsigned char>(value >> 16);
    bytes[3] = static_cast<unsigned char>(value >> 24);
    Add(bytes, 4);
}
void AddFloat(float value)
{
    std::uint32_t bits;
    std::memcpy(&bits, &value, 4);
    AddNumber(bits);
}
// A string with its end.
void AddString(const char* text)
{
    Add(text, std::strlen(text) + 1);
}

// A name with its end, where it is UTF-8; a mark in its place where it is not (the port keeps
// names as strings, and a name that is not UTF-8 loses the bytes that are not).
void AddName(const char* text)
{
    const auto* s = reinterpret_cast<const unsigned char*>(text);
    bool valid = true;
    for (std::size_t i = 0; s[i] && valid;)
    {
        const unsigned char b = s[i];
        std::size_t more;
        std::uint32_t low = 0x80, high = 0xbf;
        if (b < 0x80)
            more = 0;
        else if (b >= 0xc2 && b <= 0xdf)
            more = 1;
        else if (b >= 0xe0 && b <= 0xef)
        {
            more = 2;
            if (b == 0xe0)
                low = 0xa0;
            if (b == 0xed)
                high = 0x9f;
        }
        else if (b >= 0xf0 && b <= 0xf4)
        {
            more = 3;
            if (b == 0xf0)
                low = 0x90;
            if (b == 0xf4)
                high = 0x8f;
        }
        else
        {
            valid = false;
            break;
        }
        ++i;
        for (std::size_t k = 0; k < more; ++k, ++i)
        {
            if (s[i] < low || s[i] > high)
            {
                valid = false;
                break;
            }
            low = 0x80;
            high = 0xbf;
        }
    }
    if (valid)
        AddString(text);
    else
        AddNumber(0xfffffffdu);
}

std::size_t TypeSize(int pixel_type)
{
    return pixel_type == TINYEXR_PIXELTYPE_HALF ? 2 : 4;
}

// The values of one channel, `w` by `h` of them, of one of four kinds: noise, a ramp, one
// value, values such as a picture has.
std::vector<unsigned char> MakeValues(int pixel_type, int w, int h)
{
    const std::uint32_t kind = Next() % 4;
    const std::uint32_t first = Next();
    const std::uint32_t step_x = Next() % 8;
    const std::uint32_t step_y = Next() % 8;
    const std::size_t size = TypeSize(pixel_type);
    std::vector<unsigned char> bytes(static_cast<std::size_t>(w) * static_cast<std::size_t>(h) * size);
    std::size_t at = 0;
    for (int y = 0; y < h; ++y)
    {
        for (int x = 0; x < w; ++x)
        {
            std::uint32_t value;
            switch (kind)
            {
                case 0: value = Next(); break;
                case 1: value = first + static_cast<std::uint32_t>(x) * step_x + static_cast<std::uint32_t>(y) * step_y; break;
                case 2: value = first; break;
                default:
                {
                    const std::uint32_t n = Next();
                    value = pixel_type == TINYEXR_PIXELTYPE_FLOAT ? 0x3c000000u + n % 0x04000000u : pixel_type == TINYEXR_PIXELTYPE_HALF ? 0x3000u + n % 0x1000u : n % 1000u;
                    break;
                }
            }
            bytes[at++] = static_cast<unsigned char>(value);
            bytes[at++] = static_cast<unsigned char>(value >> 8);
            if (size == 4)
            {
                bytes[at++] = static_cast<unsigned char>(value >> 16);
                bytes[at++] = static_cast<unsigned char>(value >> 24);
            }
        }
    }
    return bytes;
}

struct Channel
{
    std::string name;
    int memory_type; // the type of the values handed to the writer
    int file_type;   // the type they are stored as
};

struct Attribute
{
    std::string name, type;
    std::vector<unsigned char> value;
};

struct Level
{
    int level_x, level_y, width, height, tiles_x, tiles_y;
    std::vector<std::vector<std::vector<unsigned char>>> tiles; // of each tile, of each channel
};

// A made-up image and how to save it.
struct Made
{
    int width = 0, height = 0;
    bool tiled = false;
    int tile_size_x = 0, tile_size_y = 0, level_mode = 0, rounding_mode = 0;
    bool without_last_level = false;
    int compression = 0;
    int long_name = 0;
    std::vector<Channel> channels;
    std::vector<std::vector<unsigned char>> images; // of each channel, for scanlines
    std::vector<Level> levels;
    std::vector<Attribute> attributes;
    // Whether the count of its chunks is to be among its attributes, as it is or wrong.
    bool with_chunk_count = false;
    std::uint32_t wrong_chunk_count = 0;
    // Whether tinyexr has to read what it saves of it: not with attributes that say other
    // than the image is.
    bool plain = true;
};

const char* const kNames[8] = { "A", "B", "G", "R", "Z", "id", "light.B", "light.G" };

// The count of its chunks into the attribute for it, if it has one: the count, or another.
void SetChunkCount(Made& m, std::uint32_t count)
{
    for (Attribute& attribute : m.attributes)
    {
        if (attribute.name == "chunkCount")
        {
            std::uint32_t value = count;
            if (m.wrong_chunk_count)
            {
                const std::uint32_t other = m.wrong_chunk_count - 1;
                value = other >= count ? other + 1 : other;
            }
            attribute.value[0] = static_cast<unsigned char>(value);
            attribute.value[1] = static_cast<unsigned char>(value >> 8);
            attribute.value[2] = static_cast<unsigned char>(value >> 16);
            attribute.value[3] = static_cast<unsigned char>(value >> 24);
            if (value != count)
                m.plain = false;
        }
    }
}

Made MakeImage(bool small)
{
    Made m;
    m.tiled = Next() % 3 == 0;
    const std::uint32_t size_kind = small ? 0 : Next() % 8;
    if (size_kind < 5)
    {
        m.width = 1 + static_cast<int>(Next() % (small ? 10 : 40));
        m.height = 1 + static_cast<int>(Next() % (small ? 36 : 40));
    }
    else if (size_kind < 7)
    {
        m.width = 1 + static_cast<int>(Next() % 150);
        m.height = 1 + static_cast<int>(Next() % 150);
    }
    else
    {
        m.width = 1 + static_cast<int>(Next() % 400);
        m.height = 1 + static_cast<int>(Next() % 100);
    }

    // Its channels: red, green, blue and alpha; without alpha; one channel; or some of eight.
    std::uint32_t count, start;
    const std::uint32_t names_kind = Next() % 4;
    if (names_kind == 0)
    {
        count = 4;
        start = 0;
    }
    else if (names_kind == 1)
    {
        count = 3;
        start = 1;
    }
    else if (names_kind == 2)
    {
        count = 1;
        start = 0;
    }
    else
    {
        count = 1 + Next() % 5;
        start = Next() % (8 - count + 1);
    }
    for (std::uint32_t c = 0; c < count; ++c)
    {
        Channel channel;
        channel.name = names_kind == 2 ? "Y" : kNames[start + c];
        channel.memory_type = static_cast<int>(Next() % 3);
        const std::uint32_t as_half = Next() % 2;
        channel.file_type = channel.memory_type == TINYEXR_PIXELTYPE_UINT ? TINYEXR_PIXELTYPE_UINT : as_half ? TINYEXR_PIXELTYPE_HALF : TINYEXR_PIXELTYPE_FLOAT;
        m.channels.push_back(channel);
    }

    // Its compression: one of the eight, PIZ more often; now and then one that is not written.
    const std::uint32_t compression = Next() % 20;
    m.compression = compression < 16 ? static_cast<int>(compression % 8) : compression == 16 ? TINYEXR_COMPRESSIONTYPE_ZFP : compression == 17 ? 9 : TINYEXR_COMPRESSIONTYPE_PIZ;
    m.long_name = Next() % 8 == 0;
    // Now and then attributes of its own: text; a type, a name or a count of chunks, which
    // the parts of a file of many parts have and tinyexr reads in any file; more attributes
    // than tinyexr keeps.
    if (Next() % 3 == 0)
    {
        const std::uint32_t attributes = 1 + Next() % 3;
        for (std::uint32_t j = 0; j < attributes; ++j)
        {
            const std::uint32_t kind = Next() % 16;
            Attribute attribute;
            attribute.type = "string";
            if (kind < 6)
            {
                attribute.name = kind < 4 ? "comment" : "owner";
                attribute.value.resize(Next() % 20);
                for (unsigned char& byte : attribute.value)
                    byte = static_cast<unsigned char>('a' + Next() % 26);
            }
            else if (kind < 10)
            {
                static const char* const kTypes[5] = { "scanlineimage", "tiledimage", "deeptile", "deepscanline", "other" };
                const char* type = kTypes[Next() % 5];
                attribute.name = "type";
                attribute.value.assign(type, type + std::strlen(type));
                m.plain = false;
            }
            else if (kind < 12)
            {
                attribute.name = "name";
                attribute.value.resize(1 + Next() % 8);
                for (unsigned char& byte : attribute.value)
                    byte = static_cast<unsigned char>('a' + Next() % 26);
            }
            else if (kind < 15)
            {
                // (Its value is filled in once the image is made.)
                const std::uint32_t wrong = Next() % 4;
                const std::uint32_t count = Next() % 40;
                if (m.with_chunk_count)
                    continue;
                m.with_chunk_count = true;
                m.wrong_chunk_count = wrong == 0 ? count + 1 : 0;
                attribute.name = "chunkCount";
                attribute.type = "int";
                attribute.value.resize(4);
            }
            else
            {
                const std::uint32_t many = 126 + Next() % 6;
                attribute.type = "int";
                attribute.value.assign(4, static_cast<unsigned char>(j));
                for (std::uint32_t k = 0; k < many; ++k)
                {
                    char name[8];
                    std::snprintf(name, sizeof name, "a%03u", k);
                    attribute.name = name;
                    m.attributes.push_back(attribute);
                }
                continue;
            }
            m.attributes.push_back(attribute);
        }
    }

    if (!m.tiled)
    {
        for (const Channel& channel : m.channels)
            m.images.push_back(MakeValues(channel.memory_type, m.width, m.height));
        const int lines = tinyexr::NumScanlines(m.compression);
        SetChunkCount(m, static_cast<std::uint32_t>((m.height + lines - 1) / lines));
        return m;
    }

    m.tile_size_x = 1 + static_cast<int>(Next() % static_cast<std::uint32_t>(std::min(m.width, 64)));
    m.tile_size_y = 1 + static_cast<int>(Next() % static_cast<std::uint32_t>(std::min(m.height, 64)));
    m.level_mode = static_cast<int>(Next() % 3);
    m.rounding_mode = static_cast<int>(Next() % 2);
    m.without_last_level = Next() % 16 == 0;

    // Its levels, as tinyexr counts them.
    EXRHeader header;
    InitEXRHeader(&header);
    header.data_window.max_x = m.width - 1;
    header.data_window.max_y = m.height - 1;
    header.tile_size_x = m.tile_size_x;
    header.tile_size_y = m.tile_size_y;
    header.tile_level_mode = m.level_mode;
    header.tile_rounding_mode = m.rounding_mode;
    std::vector<int> num_x_tiles, num_y_tiles;
    if (!tinyexr::PrecalculateTileInfo(num_x_tiles, num_y_tiles, &header))
    {
        std::fprintf(stderr, "tinyexr does not count the tiles of a made-up image\n");
        std::exit(2);
    }
    const int levels_x = static_cast<int>(num_x_tiles.size());
    const int levels_y = m.level_mode == TINYEXR_TILE_RIPMAP_LEVELS ? static_cast<int>(num_y_tiles.size()) : 1;
    for (int ly = 0; ly < levels_y; ++ly)
    {
        for (int lx = 0; lx < levels_x; ++lx)
        {
            Level level;
            level.level_x = lx;
            level.level_y = m.level_mode == TINYEXR_TILE_RIPMAP_LEVELS ? ly : lx;
            level.width = tinyexr::LevelSize(m.width, level.level_x, m.rounding_mode);
            level.height = tinyexr::LevelSize(m.height, level.level_y, m.rounding_mode);
            level.tiles_x = num_x_tiles[static_cast<std::size_t>(level.level_x)];
            level.tiles_y = num_y_tiles[static_cast<std::size_t>(level.level_y)];
            for (int t = 0; t < level.tiles_x * level.tiles_y; ++t)
            {
                std::vector<std::vector<unsigned char>> tile;
                for (const Channel& channel : m.channels)
                    tile.push_back(MakeValues(channel.memory_type, m.tile_size_x, m.tile_size_y));
                level.tiles.push_back(tile);
            }
            m.levels.push_back(level);
        }
    }
    std::uint32_t chunks = 0;
    for (const Level& level : m.levels)
        chunks += static_cast<std::uint32_t>(level.tiles.size());
    SetChunkCount(m, chunks);
    return m;
}

// The image saved by tinyexr: the file's bytes, or none and tinyexr's message.
std::vector<unsigned char> Save(Made& m, std::string& message)
{
    const std::size_t count = m.channels.size();
    EXRHeader header;
    InitEXRHeader(&header);
    std::vector<EXRChannelInfo> channels(count);
    std::vector<int> pixel_types(count), requested_pixel_types(count);
    for (std::size_t c = 0; c < count; ++c)
    {
        std::memset(&channels[c], 0, sizeof(EXRChannelInfo));
        std::strncpy(channels[c].name, m.channels[c].name.c_str(), 255);
        pixel_types[c] = m.channels[c].memory_type;
        requested_pixel_types[c] = m.channels[c].file_type;
    }
    header.num_channels = static_cast<int>(count);
    header.channels = channels.data();
    header.pixel_types = pixel_types.data();
    header.requested_pixel_types = requested_pixel_types.data();
    header.compression_type = m.compression;
    header.long_name = m.long_name;
    header.data_window.max_x = m.width - 1;
    header.data_window.max_y = m.height - 1;
    std::vector<EXRAttribute> attributes(m.attributes.size());
    for (std::size_t j = 0; j < attributes.size(); ++j)
    {
        std::memset(&attributes[j], 0, sizeof(EXRAttribute));
        std::strncpy(attributes[j].name, m.attributes[j].name.c_str(), 255);
        std::strncpy(attributes[j].type, m.attributes[j].type.c_str(), 255);
        attributes[j].value = m.attributes[j].value.data();
        attributes[j].size = static_cast<int>(m.attributes[j].value.size());
    }
    header.num_custom_attributes = static_cast<int>(attributes.size());
    header.custom_attributes = attributes.data();

    std::vector<EXRImage> images(m.tiled ? m.levels.size() : 1);
    std::vector<unsigned char*> pointers(count);
    std::vector<std::vector<EXRTile>> tiles(m.levels.size());
    std::vector<std::vector<std::vector<unsigned char*>>> tile_pointers(m.levels.size());
    for (EXRImage& image : images)
        InitEXRImage(&image);
    if (!m.tiled)
    {
        for (std::size_t c = 0; c < count; ++c)
            pointers[c] = m.images[c].data();
        images[0].images = pointers.data();
        images[0].width = m.width;
        images[0].height = m.height;
        images[0].num_channels = static_cast<int>(count);
    }
    else
    {
        header.tiled = 1;
        header.tile_size_x = m.tile_size_x;
        header.tile_size_y = m.tile_size_y;
        header.tile_level_mode = m.level_mode;
        header.tile_rounding_mode = m.rounding_mode;
        const std::size_t given = m.levels.size() - (m.without_last_level && m.levels.size() > 1 ? 1 : 0);
        for (std::size_t l = 0; l < m.levels.size(); ++l)
        {
            Level& level = m.levels[l];
            tiles[l].resize(level.tiles.size());
            tile_pointers[l].resize(level.tiles.size());
            for (std::size_t t = 0; t < level.tiles.size(); ++t)
            {
                EXRTile& tile = tiles[l][t];
                tile.offset_x = static_cast<int>(t) % level.tiles_x;
                tile.offset_y = static_cast<int>(t) / level.tiles_x;
                tile.level_x = level.level_x;
                tile.level_y = level.level_y;
                tile.width = std::min(m.tile_size_x, level.width - tile.offset_x * m.tile_size_x);
                tile.height = std::min(m.tile_size_y, level.height - tile.offset_y * m.tile_size_y);
                for (std::size_t c = 0; c < count; ++c)
                    tile_pointers[l][t].push_back(level.tiles[t][c].data());
                tile.images = tile_pointers[l][t].data();
            }
            images[l].tiles = tiles[l].data();
            images[l].num_tiles = static_cast<int>(tiles[l].size());
            images[l].level_x = level.level_x;
            images[l].level_y = level.level_y;
            images[l].width = level.width;
            images[l].height = level.height;
            images[l].num_channels = static_cast<int>(count);
            images[l].next_level = l + 1 < given ? &images[l + 1] : NULL;
        }
    }

    unsigned char* memory = NULL;
    const char* err = NULL;
    const std::size_t size = SaveEXRImageToMemory(&images[0], &header, &memory, &err);
    std::vector<unsigned char> file;
    // (Nothing, or one of its error numbers as a size.)
    if (size == 0 || size > (static_cast<std::size_t>(1) << 40))
    {
        message = err ? err : "";
        FreeEXRErrorMessage(err);
        return file;
    }
    file.assign(memory, memory + size);
    std::free(memory);
    return file;
}

// Where the value of an attribute starts in a file, or 0: after its name, its type and its
// size.
std::size_t FindAttribute(const std::vector<unsigned char>& file, const char* name, const char* type)
{
    std::string key = std::string(name) + '\0' + type + '\0';
    const auto found = std::search(file.begin(), file.end(), key.begin(), key.end());
    if (found == file.end())
        return 0;
    return static_cast<std::size_t>(found - file.begin()) + key.size() + 4;
}

std::uint32_t GetNumber(const std::vector<unsigned char>& file, std::size_t at)
{
    return static_cast<std::uint32_t>(file[at]) | static_cast<std::uint32_t>(file[at + 1]) << 8 | static_cast<std::uint32_t>(file[at + 2]) << 16 | static_cast<std::uint32_t>(file[at + 3]) << 24;
}

// (Nothing where the number would not be whole within the file.)
void SetNumber(std::vector<unsigned char>& file, std::size_t at, std::uint32_t value)
{
    if (at + 4 > file.size())
        return;
    file[at] = static_cast<unsigned char>(value);
    file[at + 1] = static_cast<unsigned char>(value >> 8);
    file[at + 2] = static_cast<unsigned char>(value >> 16);
    file[at + 3] = static_cast<unsigned char>(value >> 24);
}

void AddToNumber(std::vector<unsigned char>& file, std::size_t at, std::uint32_t by)
{
    if (at + 4 <= file.size())
        SetNumber(file, at, GetNumber(file, at) + by);
}

// Where the header of a file tinyexr saved ends, which is where its table of offsets starts;
// 0 where it does not end.
std::size_t EndOfHeader(const std::vector<unsigned char>& file)
{
    std::size_t at = 8;
    while (at < file.size() && file[at])
    {
        for (int text = 0; text < 2; ++text)
        {
            while (at < file.size() && file[at])
                ++at;
            ++at;
        }
        if (at + 4 > file.size())
            return 0;
        at += 4 + static_cast<std::size_t>(GetNumber(file, at));
    }
    return at < file.size() ? at + 1 : 0;
}

// A number such as a damaged header has: nothing, less than nothing, the most and the least
// there are, the most tinyexr takes for a width and one more, small ones of either sign.
std::uint32_t Special()
{
    const std::uint32_t kind = Next() % 8;
    const std::uint32_t small = Next() % 64;
    switch (kind)
    {
        case 0: return 0;
        case 1: return 0xffffffffu;
        case 2: return 0x7fffffffu;
        case 3: return 0x80000000u;
        case 4: return 1024 * 8192;
        case 5: return 1024 * 8192 + 1;
        case 6: return small;
        default: return 0u - small;
    }
}

#ifdef SHOW_EACH
#define STAGE(name) std::fprintf(stderr, "%s %016llx\n", name, static_cast<unsigned long long>(g_hash))
#else
#define STAGE(name)
#endif

void AddMessage(const char* err)
{
    if (err)
    {
#ifdef SHOW_REASONS
        std::fprintf(stderr, "%s\n", err);
#endif
        AddString(err);
        FreeEXRErrorMessage(err);
    }
    else
        AddNumber(0xfffffffeu);
}

struct Counts
{
    std::uint32_t headers = 0, images = 0, refused = 0, left = 0, as_rgba = 0, stopped = 0;
};

// What tinyexr reads of a file: its version, its header, its image, and it as floats of red,
// green, blue and alpha. Whether the image was read.
//
// tinyexr reads the lines of a block that is not compressed, for a channel of halves read as
// halves, without a look at the block's length, and so past the end of the file where a
// damaged block is its last. The port refuses such a block. So that the two can be compared, a
// damaged file that is not compressed is read with halves as floats, where tinyexr looks.
//
// And tinyexr throws where a damaged tile's place is past the image's edge, so that the tile
// has no width or no height: it asks a buffer of no bytes for its first. Nothing in it
// catches that, which stops a program. Here it is caught, and `stops` says which of the two
// readings it stopped (4 the image, 8 the image as red, green, blue and alpha), for the file
// of files to tell the port's side, which has to refuse such a file.
bool Read(const unsigned char* bytes, std::size_t size, std::uint32_t wish, Counts& counts, std::uint32_t& stops)
{
    bool read = false;
    bool large = false;
    EXRVersion version;
    int ret = ParseEXRVersionFromMemory(&version, bytes, size);
    AddNumber(static_cast<std::uint32_t>(ret));
    if (ret == TINYEXR_SUCCESS)
    {
        AddNumber(static_cast<std::uint32_t>(version.version));
        AddNumber(static_cast<std::uint32_t>(version.tiled));
        AddNumber(static_cast<std::uint32_t>(version.long_name));
        AddNumber(static_cast<std::uint32_t>(version.non_image));
        AddNumber(static_cast<std::uint32_t>(version.multipart));

        EXRHeader header;
        InitEXRHeader(&header);
        const char* err = NULL;
        ret = ParseEXRHeaderFromMemory(&header, &version, bytes, size, &err);
        AddNumber(static_cast<std::uint32_t>(ret));
        AddMessage(err);
        if (ret == TINYEXR_SUCCESS)
        {
            ++counts.headers;
            AddFloat(header.pixel_aspect_ratio);
            AddNumber(static_cast<std::uint32_t>(header.line_order));
            AddNumber(static_cast<std::uint32_t>(header.data_window.min_x));
            AddNumber(static_cast<std::uint32_t>(header.data_window.min_y));
            AddNumber(static_cast<std::uint32_t>(header.data_window.max_x));
            AddNumber(static_cast<std::uint32_t>(header.data_window.max_y));
            AddNumber(static_cast<std::uint32_t>(header.display_window.min_x));
            AddNumber(static_cast<std::uint32_t>(header.display_window.min_y));
            AddNumber(static_cast<std::uint32_t>(header.display_window.max_x));
            AddNumber(static_cast<std::uint32_t>(header.display_window.max_y));
            AddFloat(header.screen_window_center[0]);
            AddFloat(header.screen_window_center[1]);
            AddFloat(header.screen_window_width);
            AddNumber(static_cast<std::uint32_t>(header.chunk_count));
            AddNumber(static_cast<std::uint32_t>(header.tiled));
            AddNumber(static_cast<std::uint32_t>(header.tile_size_x));
            AddNumber(static_cast<std::uint32_t>(header.tile_size_y));
            AddNumber(static_cast<std::uint32_t>(header.tile_level_mode));
            AddNumber(static_cast<std::uint32_t>(header.tile_rounding_mode));
            AddNumber(static_cast<std::uint32_t>(header.long_name));
            AddNumber(static_cast<std::uint32_t>(header.non_image));
            AddNumber(static_cast<std::uint32_t>(header.multipart));
            AddNumber(header.header_len);
            AddNumber(static_cast<std::uint32_t>(header.num_custom_attributes));
            for (int j = 0; j < header.num_custom_attributes; ++j)
            {
                AddName(header.custom_attributes[j].name);
                AddName(header.custom_attributes[j].type);
                AddNumber(static_cast<std::uint32_t>(header.custom_attributes[j].size));
                Add(header.custom_attributes[j].value, static_cast<std::size_t>(header.custom_attributes[j].size));
            }
            AddNumber(static_cast<std::uint32_t>(header.num_channels));
            for (int c = 0; c < header.num_channels; ++c)
            {
                AddName(header.channels[c].name);
                AddNumber(static_cast<std::uint32_t>(header.channels[c].pixel_type));
                AddNumber(static_cast<std::uint32_t>(header.channels[c].x_sampling));
                AddNumber(static_cast<std::uint32_t>(header.channels[c].y_sampling));
                AddNumber(header.channels[c].p_linear);
                AddNumber(static_cast<std::uint32_t>(header.pixel_types[c]));
                AddNumber(static_cast<std::uint32_t>(header.requested_pixel_types[c]));
            }
            AddNumber(static_cast<std::uint32_t>(header.compression_type));
            AddName(header.name);
            STAGE("header");

            // A damaged size can ask for more memory and time than there is; those are left,
            // but for the sizes tinyexr refuses at once. Two more are left, which stop
            // tinyexr: a tile size of 0, which it divides by, and a count of chunks in the
            // thousands of millions, which it makes a table for before it looks whether the
            // file has one.
            {
                const std::int64_t limit = 1024 * 8192;
                const std::int64_t w = static_cast<std::int64_t>(header.data_window.max_x) - header.data_window.min_x + 1;
                const std::int64_t h = static_cast<std::int64_t>(header.data_window.max_y) - header.data_window.min_y + 1;
                const std::int64_t tile_x = header.tile_size_x, tile_y = header.tile_size_y;
                const bool refused = w <= 0 || h <= 0 || w > limit || h > limit || (header.tiled && (tile_x > limit || tile_y > limit));
                if (refused)
                    large = false;
                else if (header.tiled)
                    large = w * h > (1 << 22) || tile_x <= 0 || tile_y <= 0 || tile_x * tile_y > (1 << 16) || (w / tile_x + 1) * (h / tile_y + 1) * tile_x * tile_y > (1 << 23);
                else
                    large = w * h > (1 << 22) || header.chunk_count > (1 << 24);
            }
            if (large)
                ++counts.left;
            else
            {
                if ((wish & 1) || ((wish & 2) && header.compression_type == TINYEXR_COMPRESSIONTYPE_NONE))
                {
                    for (int c = 0; c < header.num_channels; ++c)
                    {
                        if (header.pixel_types[c] == TINYEXR_PIXELTYPE_HALF)
                            header.requested_pixel_types[c] = TINYEXR_PIXELTYPE_FLOAT;
                    }
                }
                EXRImage image;
                InitEXRImage(&image);
                err = NULL;
                bool threw = false;
                try
                {
                    ret = LoadEXRImageFromMemory(&image, &header, bytes, size, &err);
                }
                catch (const std::exception&)
                {
                    threw = true;
                }
                if (threw)
                {
                    AddNumber(0xfffffffcu);
                    stops |= 4;
                    ++counts.stopped;
                }
                else
                {
                    AddNumber(static_cast<std::uint32_t>(ret));
                    AddMessage(err);
                }
                if (threw)
                    ;
                else if (ret == TINYEXR_SUCCESS)
                {
                    read = true;
                    ++counts.images;
                    for (int c = 0; c < header.num_channels; ++c)
                        AddNumber(static_cast<std::uint32_t>(header.pixel_types[c]));
                    for (const EXRImage* level = &image; level; level = level->next_level)
                    {
                        AddNumber(static_cast<std::uint32_t>(level->level_x));
                        AddNumber(static_cast<std::uint32_t>(level->level_y));
                        AddNumber(static_cast<std::uint32_t>(level->width));
                        AddNumber(static_cast<std::uint32_t>(level->height));
                        AddNumber(static_cast<std::uint32_t>(level->num_channels));
                        AddNumber(static_cast<std::uint32_t>(level->num_tiles));
                        if (level->tiles)
                        {
                            for (int t = 0; t < level->num_tiles; ++t)
                            {
                                const EXRTile& tile = level->tiles[t];
                                AddNumber(static_cast<std::uint32_t>(tile.offset_x));
                                AddNumber(static_cast<std::uint32_t>(tile.offset_y));
                                AddNumber(static_cast<std::uint32_t>(tile.level_x));
                                AddNumber(static_cast<std::uint32_t>(tile.level_y));
                                AddNumber(static_cast<std::uint32_t>(tile.width));
                                AddNumber(static_cast<std::uint32_t>(tile.height));
                                for (int c = 0; c < level->num_channels; ++c)
                                    Add(tile.images[c], static_cast<std::size_t>(header.tile_size_x) * static_cast<std::size_t>(header.tile_size_y) * TypeSize(header.requested_pixel_types[c]));
                            }
                        }
                        else
                        {
                            for (int c = 0; c < level->num_channels; ++c)
                                Add(level->images[c], static_cast<std::size_t>(level->width) * static_cast<std::size_t>(level->height) * TypeSize(header.requested_pixel_types[c]));
                        }
                    }
                    FreeEXRImage(&image);
                }
                else
                    ++counts.refused;
                STAGE("image");
            }
        }
        FreeEXRHeader(&header);
    }

    if (!large)
    {
        float* rgba = NULL;
        int width = 0, height = 0;
        const char* err = NULL;
        bool threw = false;
        try
        {
            ret = LoadEXRFromMemory(&rgba, &width, &height, bytes, size, &err);
        }
        catch (const std::exception&)
        {
            threw = true;
        }
        if (threw)
        {
            AddNumber(0xfffffffcu);
            stops |= 8;
        }
        else
        {
            AddNumber(static_cast<std::uint32_t>(ret));
            AddMessage(err);
        }
        if (!threw && ret == TINYEXR_SUCCESS)
        {
            ++counts.as_rgba;
            AddNumber(static_cast<std::uint32_t>(width));
            AddNumber(static_cast<std::uint32_t>(height));
            Add(rgba, static_cast<std::size_t>(width) * static_cast<std::size_t>(height) * 16);
            std::free(rgba);
        }
    }
    return read;
}

void WriteNumber(std::FILE* file, std::uint32_t value)
{
    unsigned char bytes[4];
    bytes[0] = static_cast<unsigned char>(value);
    bytes[1] = static_cast<unsigned char>(value >> 8);
    bytes[2] = static_cast<unsigned char>(value >> 16);
    bytes[3] = static_cast<unsigned char>(value >> 24);
    std::fwrite(bytes, 1, 4, file);
}
} // namespace

int main(int argc, char** argv)
{
    if (argc < 5 || (std::strcmp(argv[1], "tinyexr_read") != 0 && std::strcmp(argv[1], "tinyexr_read_small") != 0))
    {
        std::fprintf(stderr, "usage: dump tinyexr_read|tinyexr_read_small <seed> <count> <file>\n");
        return 2;
    }
    const bool small = std::strcmp(argv[1], "tinyexr_read_small") == 0;
    g_state = std::strtoull(argv[2], NULL, 10);
    const std::uint32_t count = static_cast<std::uint32_t>(std::strtoul(argv[3], NULL, 10));
    std::FILE* out = std::fopen(argv[4], "wb");
    if (!out)
    {
        std::fprintf(stderr, "cannot write %s\n", argv[4]);
        return 2;
    }
    Counts counts;
    std::uint64_t file_total = 0;
    for (std::uint32_t i = 0; i < count; ++i)
    {
        Made made = MakeImage(small);
        std::string message;
        std::vector<unsigned char> file = Save(made, message);
        bool sound = !file.empty() && made.plain;
        bool damaged = false;
        // (What tinyexr does not save is read as a file of no bytes.)
        if (!file.empty())
        {
            // Now and then the order of the lines named the other way, or the window moved
            // across or down (which the blocks of a file of scanlines then do not agree with).
            const std::uint32_t change = Next() % 8;
            const std::uint32_t by = Next() % 101 - 50;
            if (change == 0)
            {
                const std::size_t at = FindAttribute(file, "lineOrder", "lineOrder");
                if (at)
                    file[at] = 1;
            }
            else if (change == 1 || change == 2)
            {
                const std::size_t at = FindAttribute(file, "dataWindow", "box2i");
                if (at)
                {
                    AddToNumber(file, at + (change == 1 ? 0 : 4), by);
                    AddToNumber(file, at + (change == 1 ? 8 : 12), by);
                }
                sound = sound && (change == 1 || made.tiled || by == 0);
            }
            // A third of them damaged: bits turned over, the end cut off, a byte made up or
            // made zero, a number of the header made up, the table of offsets changed, a
            // number at the start of a block made up, or one of those a PIZ block's data
            // starts with.
            if (Next() % 3 == 0)
            {
                const std::uint32_t how = Next() % 9;
                sound = false;
                damaged = true;
                if (how == 0)
                {
                    for (std::uint32_t k = 1 + Next() % 3; k > 0; --k)
                    {
                        const std::uint32_t bit = Next() % static_cast<std::uint32_t>(file.size() * 8);
                        file[bit >> 3] ^= static_cast<unsigned char>(1u << (bit & 7));
                    }
                }
                else if (how == 1)
                {
                    // (Anywhere, or within its start, where the header and the table are.)
                    const std::uint32_t near = Next() % 2;
                    const std::uint32_t at = Next();
                    file.resize(at % (near ? std::min<std::size_t>(file.size(), 600) : file.size()));
                }
                else if (how == 2)
                {
                    const std::uint32_t at = Next() % static_cast<std::uint32_t>(file.size());
                    file[at] = static_cast<unsigned char>(Next());
                }
                else if (how == 3)
                {
                    const std::uint32_t bit = Next() % static_cast<std::uint32_t>(std::min<std::size_t>(file.size(), 400) * 8);
                    file[bit >> 3] ^= static_cast<unsigned char>(1u << (bit & 7));
                }
                else if (how == 4)
                    file[Next() % static_cast<std::uint32_t>(std::min<std::size_t>(file.size(), 400))] = 0;
                else if (how == 5)
                {
                    const std::uint32_t which = Next() % 8;
                    const std::uint32_t pick = Next();
                    const std::uint32_t value = Special();
                    if (which < 3)
                    {
                        const std::size_t at = FindAttribute(file, which < 2 ? "dataWindow" : "displayWindow", "box2i");
                        if (at)
                            SetNumber(file, at + 4 * (pick % 4), value);
                    }
                    else if (which == 3)
                    {
                        const std::size_t at = FindAttribute(file, "tiles", "tiledesc");
                        if (at && pick % 3 < 2)
                            SetNumber(file, at + 4 * (pick % 3), value);
                        else if (at)
                            file[at + 8] = static_cast<unsigned char>(pick >> 8);
                    }
                    else if (which == 4)
                    {
                        const std::size_t at = FindAttribute(file, "compression", "compression");
                        if (at)
                            file[at] = static_cast<unsigned char>(pick % 13 == 12 ? 128 : pick % 13);
                    }
                    else if (which == 5)
                    {
                        // The first channel: its name, then its type, a byte and three more,
                        // and its two samplings.
                        std::size_t at = FindAttribute(file, "channels", "chlist");
                        if (at)
                        {
                            while (file[at])
                                ++at;
                            SetNumber(file, at + 1 + (pick % 3 == 0 ? 0 : pick % 3 == 1 ? 8 : 12), (pick >> 8) % 4);
                        }
                    }
                    else if (which == 6)
                    {
                        const std::size_t at = FindAttribute(file, "chunkCount", "int");
                        if (at)
                            SetNumber(file, at, value);
                    }
                    else
                    {
                        const std::size_t at = FindAttribute(file, "lineOrder", "lineOrder");
                        if (at)
                            file[at] = static_cast<unsigned char>(pick % 4);
                    }
                }
                else
                {
                    // The table of offsets comes after the header: one of its first six
                    // made zero, put near the end, made up, or changed for the one after
                    // it; or a number at the start of the block it leads to made up.
                    const std::size_t table = EndOfHeader(file);
                    const std::uint32_t entry = Next() % 6;
                    const std::uint32_t pick = Next();
                    const std::uint32_t value = Special();
                    const std::size_t at = table + 8 * entry;
                    if (table && at + 16 <= file.size())
                    {
                        if (how == 6)
                        {
                            switch (pick % 4)
                            {
                                case 0:
                                    SetNumber(file, at, 0);
                                    SetNumber(file, at + 4, 0);
                                    break;
                                case 1:
                                    SetNumber(file, at, static_cast<std::uint32_t>(file.size()) - (pick >> 8) % 16);
                                    break;
                                case 2:
                                    SetNumber(file, at, (pick >> 8) % static_cast<std::uint32_t>(file.size()));
                                    break;
                                default:
                                    for (std::size_t k = 0; k < 8; ++k)
                                        std::swap(file[at + k], file[at + 8 + k]);
                                    break;
                            }
                        }
                        else
                        {
                            const std::size_t block = GetNumber(file, at);
                            if (GetNumber(file, at + 4) == 0 && block + 20 <= file.size())
                            {
                                if (how == 7 || made.compression != TINYEXR_COMPRESSIONTYPE_PIZ)
                                    SetNumber(file, block + 4 * (pick % 5), value);
                                else
                                {
                                    // A PIZ block: after the numbers it starts with, the
                                    // least and the most of its values, a bitmap from the one
                                    // to the other, a length, and the five numbers the
                                    // Huffman coder starts with: one of the six made up.
                                    std::size_t data = block + (made.tiled ? 20 : 8);
                                    if (data + 4 <= file.size())
                                    {
                                        const std::uint32_t least = static_cast<std::uint32_t>(file[data]) | static_cast<std::uint32_t>(file[data + 1]) << 8;
                                        const std::uint32_t most = static_cast<std::uint32_t>(file[data + 2]) | static_cast<std::uint32_t>(file[data + 3]) << 8;
                                        data += 4 + (least <= most ? most - least + 1 : 0);
                                        SetNumber(file, data + 4 * (pick % 6), (pick >> 8) % 4 == 0 ? value : (pick >> 10) % 70000);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // What to read it as: halves as they are, or as floats; and whether it was damaged.
        const std::uint32_t wish = (Next() % 2) | (damaged ? 2u : 0u);
        file_total += file.size();

        AddNumber(static_cast<std::uint32_t>(file.size()));
        Add(file.data(), file.size());
        // (Read from memory of just the file's size, so that a sanitizer sees a read past
        // its end. A file of no bytes is still a file: tinyexr has other answers for no
        // pointer.)
        unsigned char* exact = static_cast<unsigned char*>(std::malloc(file.empty() ? 1 : file.size()));
        if (!exact)
            return 2;
        if (!file.empty())
            std::memcpy(exact, file.data(), file.size());
        std::uint32_t stops = 0;
        const bool read = Read(exact, file.size(), wish, counts, stops);
        std::free(exact);

        WriteNumber(out, wish | stops);
        WriteNumber(out, static_cast<std::uint32_t>(file.size()));
        if (!file.empty())
            std::fwrite(file.data(), 1, file.size(), out);
#ifdef SHOW_EACH
        std::fprintf(stderr, "file %u %016llx\n", i, static_cast<unsigned long long>(g_hash));
#endif
        // A sound file has to be read.
        if (sound && !read)
        {
            std::fprintf(stderr, "%s: file %u is sound and tinyexr does not read it\n", argv[1], i);
            return 2;
        }

        if ((i + 1) % 200 == 0 || i + 1 == count)
            std::printf("%s %u %016llx\n", argv[1], i + 1, static_cast<unsigned long long>(g_hash));
    }
    if (std::fclose(out) != 0)
    {
        std::fprintf(stderr, "cannot write %s\n", argv[4]);
        return 2;
    }
    std::printf("%s bytes of files: %llu; headers read: %u, images read: %u, refused: %u, left: %u, where tinyexr stops: %u; as red, green, blue and alpha: %u\n", argv[1], static_cast<unsigned long long>(file_total), counts.headers, counts.images, counts.refused, counts.left, counts.stopped, counts.as_rgba);
    return 0;
}
