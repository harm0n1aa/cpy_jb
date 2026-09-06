export TARGET = iphone:clang:latest:14.0
# Procursus/Sileo APT rejects data.tar.lzma. xz is what stock packages use.
export THEOS_PLATFORM_DEB_COMPRESSION_TYPE = xz
export THEOS_PLATFORM_DEB_COMPRESSION_LEVEL = 6

include $(THEOS)/makefiles/common.mk

SUBPROJECTS = daemon ctl tweak

include $(THEOS)/makefiles/aggregate.mk
