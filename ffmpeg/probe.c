#include <stdio.h>
#include <string.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/avutil.h>

static const char *const wanted_encoders[] = {
    "h264_nvenc",   "hevc_nvenc",        "av1_nvenc",
    "h264_amf",     "hevc_amf",          "av1_amf",
    "h264_qsv",     "hevc_qsv",          "av1_qsv",
    "h264_vaapi",   "hevc_vaapi",        "av1_vaapi",
    "h264_vulkan",  "hevc_vulkan",       "av1_vulkan",
    "h264_mf",      "hevc_mf",           "av1_mf",
    "h264_d3d12va", "hevc_d3d12va",      "av1_d3d12va",
    "h264_videotoolbox", "hevc_videotoolbox",
    "libopenh264",  "aac",               "aac_at",
    "libopus",
};

static const char *const required_decoders[] = {"h264", "hevc", "av1", "aac", "opus"};
static const char *const required_muxers[] = {"mp4", "matroska"};
static const char *const required_demuxers[] = {"mov", "matroska"};

#define COUNT(array) (sizeof(array) / sizeof((array)[0]))

static int missing(const char *kind, const char *name, int present)
{
    if (present)
        return 0;
    fprintf(stderr, "missing %s: %s\n", kind, name);
    return 1;
}

int main(int argc, char **argv)
{
    int failures = 0;
    size_t i;
    int arg;

    printf("version: %s\n", av_version_info());
    printf("licence: %s\n", avcodec_license());
    if (strncmp(avcodec_license(), "LGPL", 4) != 0 ||
        strncmp(avformat_license(), "LGPL", 4) != 0 ||
        strncmp(avutil_license(), "LGPL", 4) != 0) {
        fprintf(stderr, "the libraries are not LGPL\n");
        failures++;
    }

    for (i = 0; i < COUNT(wanted_encoders); i++)
        printf("encoder %-18s %s\n", wanted_encoders[i],
               avcodec_find_encoder_by_name(wanted_encoders[i]) ? "present" : "absent");

    for (arg = 1; arg < argc; arg++)
        failures += missing("encoder", argv[arg], avcodec_find_encoder_by_name(argv[arg]) != NULL);
    for (i = 0; i < COUNT(required_decoders); i++)
        failures += missing("decoder", required_decoders[i],
                            avcodec_find_decoder_by_name(required_decoders[i]) != NULL);
    for (i = 0; i < COUNT(required_muxers); i++)
        failures += missing("muxer", required_muxers[i],
                            av_guess_format(required_muxers[i], NULL, NULL) != NULL);
    for (i = 0; i < COUNT(required_demuxers); i++)
        failures += missing("demuxer", required_demuxers[i],
                            av_find_input_format(required_demuxers[i]) != NULL);

    return failures == 0 ? 0 : 1;
}
