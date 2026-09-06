export TARGET = iphone:clang:latest:14.0
# Procursus/Sileo APT rejects data.tar.lzma. xz is what stock packages use.
export THEOS_PLATFORM_DEB_COMPRESSION_TYPE = xz
export THEOS_PLATFORM_DEB_COMPRESSION_LEVEL = 6
# Fat arm64+arm64e binaries still ship as iphoneos-arm64 for Dopamine/Sileo.
export THEOS_PACKAGE_ARCH = iphoneos-arm64

include $(THEOS)/makefiles/common.mk

SUBPROJECTS = daemon ctl tweak

include $(THEOS)/makefiles/aggregate.mk
