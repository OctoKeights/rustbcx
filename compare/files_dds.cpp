// Reads .dds files with tinyddsloader and prints what it finds in each, in the form
// examples/files.rs prints what it wrote. compare/files.sh compares the two.
//
//   files-dds <folder> <name> [<name> ...]
#define TINYDDSLOADER_IMPLEMENTATION
#include "tinyddsloader.h"

#include <cstdio>
#include <fstream>
#include <iterator>
#include <string>
#include <vector>

static unsigned long long Fnv(const unsigned char* bytes, size_t count)
{
    unsigned long long hash = 14695981039346656037ull;
    for (size_t i = 0; i < count; ++i)
    {
        hash ^= bytes[i];
        hash *= 1099511628211ull;
    }
    return hash;
}

int main(int argc, char** argv)
{
    for (int a = 2; a < argc; ++a)
    {
        const std::string name = argv[a];
        std::ifstream in(std::string(argv[1]) + "/" + name + ".dds", std::ios::binary);
        std::vector<unsigned char> data((std::istreambuf_iterator<char>(in)), std::istreambuf_iterator<char>());
        tinyddsloader::DDSFile dds;
        const auto result = dds.Load(data.data(), data.size());
        if (result != tinyddsloader::Result::Success)
        {
            std::printf("%s NOT READ (%d)\n", name.c_str(), static_cast<int>(result));
            continue;
        }
        const bool cube = dds.IsCubemap();
        const bool volume = dds.GetTextureDimension() == tinyddsloader::DDSFile::TextureDimension::Texture3D;
        const unsigned images = dds.GetArraySize();
        std::printf("%s %ux%ux%u layers %u cube %d mips %u\n", name.c_str(), dds.GetWidth(), dds.GetHeight(), volume ? dds.GetDepth() : 1, cube ? images / 6 : images,
                    cube ? 1 : 0, dds.GetMipCount());
        for (unsigned mip = 0; mip < dds.GetMipCount(); ++mip)
        {
            unsigned index = 0;
            for (unsigned image = 0; image < images; ++image)
            {
                const auto* level = dds.GetImageData(mip, image);
                for (unsigned slice = 0; slice < level->m_depth; ++slice)
                    std::printf("%s mip %u image %u %016llx\n", name.c_str(), mip, index++,
                                Fnv(static_cast<const unsigned char*>(level->m_mem) + static_cast<size_t>(slice) * level->m_memSlicePitch, level->m_memSlicePitch));
            }
        }
    }
    return 0;
}
