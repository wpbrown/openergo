TARGET    = openergo
QT       += multimedia
CONFIG   += console c++11 warn_on
TEMPLATE  = app

SOURCES  += main.cpp \
            tracker.cpp \
            interactionsource.cpp

HEADERS  += tracker.h \
            interactionsource.h

unix {
    SOURCES   += interactionsource_unix.cpp
    CONFIG    += x11 link_pkgconfig
    QT        += x11extras
    PKGCONFIG += xi xtst

    packagesExist(xcb-xinput) {
        DEFINES     += HAS_XCB_XINPUT
        INCLUDEPATH += $$system(pkg-config --cflags-only-I xcb-xinput)
    } else {
        HEADERS += xcb_xinput_raw.h
    }

} else:win32 {
    SOURCES += interactionsource_win32.cpp
    QT      -= gui
    DEFINES += WIN32_LEAN_AND_MEAN
    LIBS    += "$$(PROGRAMFILES)/Microsoft SDKs/Windows/v7.1A/Lib/User32.lib"
}

RESOURCES += resources/resources.qrc
