// Reads .ktx and .ktx2 files with the Khronos library (libktx, of KTX-Software) and prints what
// it finds in each, in the form examples/files.rs prints what it wrote. compare/files.sh
// compares the two.
//
//   files-ktx <folder> <ktx or ktx2> <name> [<name> ...]
#include <ktx.h>
#include <stdio.h>
#include <string.h>

static unsigned long long fnv(const unsigned char* b, size_t n) {
    unsigned long long h = 14695981039346656037ull;
    for (size_t i = 0; i < n; ++i) { h ^= b[i]; h *= 1099511628211ull; }
    return h;
}

int main(int argc, char** argv) {
    for (int a = 3; a < argc; ++a) {
        char path[1024];
        snprintf(path, sizeof path, "%s/%s.%s", argv[1], argv[a], argv[2]);
        ktxTexture* t = NULL;
        KTX_error_code e = ktxTexture_CreateFromNamedFile(path, KTX_TEXTURE_CREATE_LOAD_IMAGE_DATA_BIT, &t);
        if (e != KTX_SUCCESS) { printf("%s NOT READ (%s)\n", argv[a], ktxErrorString(e)); continue; }
        int volume = t->numDimensions == 3;
        printf("%s %ux%ux%u layers %u cube %d mips %u\n", argv[a], t->baseWidth, t->baseHeight, volume ? t->baseDepth : 1, t->numLayers, t->isCubemap ? 1 : 0, t->numLevels);
        if ((t->isArray != 0) != (strstr(argv[a], "array") || strstr(argv[a], "cubes"))) printf("%s isArray %d\n", argv[a], t->isArray);
        for (unsigned level = 0; level < t->numLevels; ++level) {
            unsigned index = 0;
            unsigned depth = volume ? (t->baseDepth >> level ? t->baseDepth >> level : 1) : 1;
            size_t size = ktxTexture_GetImageSize(t, level);
            for (unsigned layer = 0; layer < t->numLayers; ++layer)
                for (unsigned fs = 0; fs < (t->isCubemap ? 6 : depth); ++fs) {
                    ktx_size_t offset;
                    e = ktxTexture_GetImageOffset(t, level, layer, fs, &offset);
                    if (e != KTX_SUCCESS) { printf("%s mip %u image %u NO OFFSET (%s)\n", argv[a], level, index++, ktxErrorString(e)); continue; }
                    printf("%s mip %u image %u %016llx\n", argv[a], level, index++, fnv(ktxTexture_GetData(t) + offset, size));
                }
        }
        ktxTexture_Destroy(t);
    }
    return 0;
}
