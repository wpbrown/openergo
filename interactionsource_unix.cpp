#include "interactionsource.h"

#include <QDebug>
#include <QThread>
#include <QX11Info>
#include <cmath>

#include <X11/Xlib.h>
#include <X11/keysym.h>
#include <X11/extensions/XInput2.h>
#include <X11/extensions/XTest.h>
#include <X11/extensions/XI2proto.h>
#include <xcb/xcb.h>

#ifdef HAS_XCB_XINPUT
    #include <xcb/xinput.h>
#else
    #include "xcb_xinput_raw.h"
#endif

namespace {
KeyCode metaCode;
KeyCode aCode;
KeyCode zCode;
KeyCode xCode;

uint getPixelsMoved(xcb_input_raw_motion_event_t *event) {
    void *mask = event + 1;
    FP3232 *values = (FP3232 *)((unsigned char *)mask + (event->valuators_len * 4));

    uint pixels = 0;
    for (int i = 0; i < event->valuators_len * 4 * 8; ++i) {
        if (XIMaskIsSet(mask, i)) {
            pixels += std::abs((values++)->integral);
        }
    }
    return pixels;
}

}

InteractionSource::InteractionSource()
{
    _timer.setSingleShot(true);
    connect(&_timer, &QTimer::timeout, this, &InteractionSource::_timerExpired);

    Display *display = QX11Info::display();

    int xi_opcode, event, error;
    if (!XQueryExtension(display, "XInputExtension", &xi_opcode, &event, &error))
        qFatal("X Input extension not available.\n");

    int major, minor;
    if (!XTestQueryExtension(display, &event, &error, &major, &minor))
        qFatal("X Test extension not available.\n");

    qDebug() << "X Input version ";
    qDebug() << "X Test version " << major << minor;

    XIEventMask mask;
    unsigned char maskData[XIMaskLen(XI_RawMotion)] = {};
    mask.deviceid = XIAllMasterDevices;
    mask.mask_len = sizeof(maskData);
    mask.mask = maskData;
    XISetMask(mask.mask, XI_RawKeyPress);
    XISetMask(mask.mask, XI_RawKeyRelease);
    XISetMask(mask.mask, XI_RawButtonPress);
    XISetMask(mask.mask, XI_RawButtonRelease);
    XISetMask(mask.mask, XI_RawMotion);

    XISelectEvents(display, DefaultRootWindow(display), &mask, 1);
    XSync(display, True);

    XGrabKey(display, XKeysymToKeycode(display, XK_a), Mod4Mask, DefaultRootWindow(display), false, GrabModeAsync, GrabModeAsync);
    XGrabKey(display, XKeysymToKeycode(display, XK_z), Mod4Mask, DefaultRootWindow(display), false, GrabModeAsync, GrabModeAsync);
    XGrabKey(display, XKeysymToKeycode(display, XK_x), Mod4Mask, DefaultRootWindow(display), false, GrabModeAsync, GrabModeAsync);

    metaCode = XKeysymToKeycode(display, XK_Super_L);
    aCode = XKeysymToKeycode(display, XK_a);
    zCode = XKeysymToKeycode(display, XK_z);
    xCode = XKeysymToKeycode(display, XK_x);
}

InteractionSource::~InteractionSource()
{
}

void
InteractionSource::simulateClick(Qt::MouseButton button, ButtonAction action)
{
    Display *display = QX11Info::display();

    uint buttonCode;
    if (button == Qt::LeftButton)
        buttonCode = Button1;
    else if (button == Qt::RightButton)
        buttonCode = Button3;
    else
        throw 0;

    uint actionCount = 1;
    if (action & ButtonDoubleClick) {
        action = ButtonClick;
        actionCount = 2;
    }

    uint eventCount = 0;
    for (uint i = 0; i < actionCount; ++i) {
        if (action & ButtonDown)
            XTestFakeButtonEvent(display, buttonCode, true, eventCount++ * 50);
        if (action & ButtonUp)
            XTestFakeButtonEvent(display, buttonCode, false, eventCount++ * 50);
    }

    XFlush(display);
}

bool
InteractionSource::nativeEventFilter(const QByteArray &, void *message, long *)
{
    xcb_generic_event_t* event = static_cast<xcb_generic_event_t *>(message);
    u_int8_t responseType = event->response_type & ~0x80;
    if (responseType == XCB_GE_GENERIC) {
        xcb_ge_generic_event_t *geEvent = static_cast<xcb_ge_generic_event_t *>(message);

        if (geEvent->event_type == XCB_INPUT_RAW_MOTION) {
            auto *motionEvent = static_cast<xcb_input_raw_motion_event_t *>(message);
            uint distance = getPixelsMoved(motionEvent);
            _distanceSinceLastTimer += distance;
            if (_leftButtonDown)
                _distanceSinceButtonDown += distance;

            if (_distanceSinceButtonDown > 10)
                emit userDragged();

            _timer.start(350);
        } else if (geEvent->event_type == XCB_INPUT_RAW_BUTTON_PRESS || geEvent->event_type == XCB_INPUT_RAW_BUTTON_RELEASE) {
            auto *buttonEvent = static_cast<xcb_input_raw_button_press_event_t *>(message);
            bool pressed = geEvent->event_type == XCB_INPUT_RAW_BUTTON_PRESS;
            if (buttonEvent->detail == 1) {
                _leftButtonDown = pressed;
                if (!pressed)
                    _distanceSinceButtonDown = 0;
            }

            if (pressed) {
                if (buttonEvent->sourceid == 4) {
                    qDebug() << "ignored autoclick";
                    return false;
                }
                qDebug() << "Click canceling next dwell.";
                _timer.stop();
                emit userClicked();
            }
        } else if (geEvent->event_type == XCB_INPUT_RAW_KEY_PRESS || geEvent->event_type == XCB_INPUT_RAW_KEY_RELEASE) {
            auto *keyEvent = static_cast<xcb_input_raw_key_press_event_t *>(message);
            bool pressed = geEvent->event_type == XCB_INPUT_RAW_KEY_PRESS;

            bool metaKey = metaCode == keyEvent->detail;
            bool commandKey =
                xCode == keyEvent->detail || zCode == keyEvent->detail || aCode == keyEvent->detail;

            if (pressed) {
                if (metaKey) {
                    _metaDown = true;
                    _xDown = false;
                    _zDown = false;
                } else if (_metaDown && commandKey) {
                    _xDown = xCode == keyEvent->detail;
                    _zDown = zCode == keyEvent->detail;

                    if (aCode == keyEvent->detail) {
                        emit dragShortcutPressed();
                    }
                }
            } else {
                if (metaKey) {
                    _metaDown = false;
                    _xDown = false;
                    _zDown = false;
                }
            }

            qDebug() << _metaDown << _xDown << _zDown;
        }
        return true;
    }
    return false;
}


