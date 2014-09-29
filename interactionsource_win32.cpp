#include "interactionsource.h"
#include <QDebug>

#include <Windows.h>
#include <QCoreApplication>
#include <strsafe.h>

void ErrorExit(LPTSTR lpszFunction)
{
    // Retrieve the system error message for the last-error code

    LPVOID lpMsgBuf;
    LPVOID lpDisplayBuf;
    DWORD dw = GetLastError();

    FormatMessage(
        FORMAT_MESSAGE_ALLOCATE_BUFFER |
        FORMAT_MESSAGE_FROM_SYSTEM |
        FORMAT_MESSAGE_IGNORE_INSERTS,
        NULL,
        dw,
        MAKELANGID(LANG_NEUTRAL, SUBLANG_DEFAULT),
        (LPTSTR) &lpMsgBuf,
        0, NULL );

    // Display the error message and exit the process

    lpDisplayBuf = (LPVOID)LocalAlloc(LMEM_ZEROINIT,
        (lstrlen((LPCTSTR)lpMsgBuf) + lstrlen((LPCTSTR)lpszFunction) + 40) * sizeof(TCHAR));
    StringCchPrintf((LPTSTR)lpDisplayBuf,
        LocalSize(lpDisplayBuf) / sizeof(TCHAR),
        TEXT("%s failed with error %d: %s"),
        lpszFunction, dw, lpMsgBuf);
    MessageBox(NULL, (LPCTSTR)lpDisplayBuf, TEXT("Error"), MB_OK);

    LocalFree(lpMsgBuf);
    LocalFree(lpDisplayBuf);
    ExitProcess(dw);
}

InteractionSource::InteractionSource()
{
    _timer.setSingleShot(true);
    connect(&_timer, &QTimer::timeout, this, &InteractionSource::_timerExpired);

    HINSTANCE hInst = GetModuleHandle(NULL);
    WNDCLASS wc = {};
    wc.hInstance = hInst;
    wc.lpfnWndProc = &DefWindowProc;
    wc.lpszClassName = L"rih";

    ATOM rihClass = RegisterClass(&wc);
    if (rihClass == 0) {
        qCritical() << "reg class failed";
    }
    HWND hwnd = CreateWindow(wc.lpszClassName,NULL,0,0,0,0,0,HWND_MESSAGE,NULL,hInst,NULL);
    if (hwnd == NULL) {
        ErrorExit(L"CreateWindow");
        qCritical() << "creawin failed";
    }

    RegisterHotKey(hwnd, 1, MOD_NOREPEAT | MOD_WIN, 'A');
    RegisterHotKey(hwnd, 2, MOD_NOREPEAT | MOD_WIN, 'Z');
    RegisterHotKey(hwnd, 3, MOD_NOREPEAT | MOD_WIN, 'C');

    RAWINPUTDEVICE rid[2];
    rid[0].usUsagePage = 0x01;
    rid[0].usUsage = 0x06;
    rid[0].dwFlags = RIDEV_NOLEGACY | RIDEV_INPUTSINK;
    rid[0].hwndTarget = hwnd;

    rid[1].usUsagePage = 0x01;
    rid[1].usUsage = 0x02;
    rid[1].dwFlags = RIDEV_NOLEGACY | RIDEV_INPUTSINK;
    rid[1].hwndTarget = hwnd;

    if (RegisterRawInputDevices(rid, 2, sizeof(rid[0])) == FALSE) {
        qCritical() << "reg failed";
    }
}

InteractionSource::~InteractionSource()
{
}

void
InteractionSource::simulateClick(Qt::MouseButton button, ButtonAction action)
{
    bool leftButton = (button == Qt::LeftButton);
    DWORD dwFlagsDown = leftButton ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_RIGHTDOWN;
    DWORD dwFlagsUp   = leftButton ? MOUSEEVENTF_LEFTUP : MOUSEEVENTF_RIGHTUP;

    if (action & ButtonDown || action & ButtonDoubleClick)
        mouse_event(dwFlagsDown, 0, 0, 0, 0);
    if (action & ButtonUp || action & ButtonDoubleClick)
        mouse_event(dwFlagsUp, 0, 0, 0, 0);

    if (action & ButtonDoubleClick) {
        mouse_event(dwFlagsDown, 0, 0, 0, 0);
        mouse_event(dwFlagsUp, 0, 0, 0, 0);
    }
}

bool
InteractionSource::nativeEventFilter(const QByteArray &, void *message, long *result)
{
    static LPBYTE inputBuffer = NULL;
    static UINT inputBufferSize = 0;

    MSG &msg = *static_cast<MSG *>(message);
    if (msg.message == WM_INPUT) {
        UINT dwSize;
        HRESULT hResult;
        TCHAR szTempOutput[1024];
        GetRawInputData((HRAWINPUT)msg.lParam, RID_INPUT, NULL, &dwSize, sizeof(RAWINPUTHEADER));
        if (dwSize > inputBufferSize) {
            delete[] inputBuffer;
            inputBuffer = new BYTE[dwSize];
            inputBufferSize = dwSize;
        }

        if (GetRawInputData((HRAWINPUT)msg.lParam, RID_INPUT, inputBuffer, &dwSize, sizeof(RAWINPUTHEADER)) != dwSize )
             OutputDebugString (TEXT("GetRawInputData does not return correct size !\n"));

        RAWINPUT* raw = (RAWINPUT*)inputBuffer;

        if (raw->header.dwType == RIM_TYPEKEYBOARD)
        {
            hResult = StringCchPrintf(szTempOutput, STRSAFE_MAX_CCH, TEXT(" Kbd: make=%04x Flags:%04x Reserved:%04x ExtraInformation:%08x, msg=%04x VK=%04x \n"),
                raw->data.keyboard.MakeCode,
                raw->data.keyboard.Flags,
                raw->data.keyboard.Reserved,
                raw->data.keyboard.ExtraInformation,
                raw->data.keyboard.Message,
                raw->data.keyboard.VKey);
            if (FAILED(hResult))
            {
            // TODO: write error handler
            }
            OutputDebugString(szTempOutput);

            bool pressed = !(raw->data.keyboard.Flags & RI_KEY_BREAK);


            bool metaKey = raw->data.keyboard.VKey == VK_LWIN || raw->data.keyboard.VKey == VK_RWIN;

            bool commandKey =
                raw->data.keyboard.VKey == 'C' || raw->data.keyboard.VKey == 'Z' || raw->data.keyboard.VKey == 'A';

            //qDebug() << "pressed" << pressed << "metakey" << metaKey << "a" << (raw->data.keyboard.VKey == 'A');

            if (!metaKey) {
                qDebug() << "PRESSED" << "(metadown?" << _metaDown << ")";
                if (_metaDown && commandKey) {
                    qDebug() << "METADOWN && COMMANDKEY";
                    _xDown = raw->data.keyboard.VKey == 'C';
                    _zDown = raw->data.keyboard.VKey == 'Z';

                    if (raw->data.keyboard.VKey == 'A') {
                        qDebug() << "A";
                        emit dragShortcutPressed();
                    }
                } else
                    qDebug() << "NOPE";
            } else {
                if (pressed) {
                    _metaDown = true;
                    _xDown = false;
                    _zDown = false;
                } else{
                    _metaDown = false;
                    _xDown = false;
                    _zDown = false;
                }
            }

            qDebug() << _metaDown << _xDown << _zDown;
        }
        else if (raw->header.dwType == RIM_TYPEMOUSE)
        {
            hResult = StringCchPrintf(szTempOutput, STRSAFE_MAX_CCH, TEXT("Mouse: usFlags=%04x ulButtons=%04x usButtonFlags=%04x usButtonData=%04x ulRawButtons=%04x lLastX=%04d lLastY=%04d ulExtraInformation=%04x\r\n"),
                raw->data.mouse.usFlags,
                raw->data.mouse.ulButtons,
                raw->data.mouse.usButtonFlags,
                raw->data.mouse.usButtonData,
                raw->data.mouse.ulRawButtons,
                raw->data.mouse.lLastX,
                raw->data.mouse.lLastY,
                raw->data.mouse.ulExtraInformation);

            if (FAILED(hResult))
            {
            // TODO: write error handler
            }
            OutputDebugString(szTempOutput);
            if (raw->data.mouse.usButtonFlags & RI_MOUSE_LEFT_BUTTON_DOWN) {
                _leftButtonDown = true;
            } else if (raw->data.mouse.usButtonFlags & RI_MOUSE_LEFT_BUTTON_UP || raw->data.mouse.usButtonFlags & RI_MOUSE_RIGHT_BUTTON_UP) {
                if (raw->data.mouse.usButtonFlags & RI_MOUSE_LEFT_BUTTON_UP) {
                    _leftButtonDown = false;
                    _distanceSinceButtonDown = 0;
                }
                if (raw->header.hDevice == NULL) {
                    qDebug() << "ignored autoclick.";
                } else {
                    qDebug() << "Click canceling next dwell.";
                    _timer.stop();
                    emit userClicked();
                }
            } else if (!raw->data.mouse.usButtonFlags) {
                if (raw->data.mouse.usFlags) {
                    if (raw->data.mouse.usFlags & MOUSE_MOVE_ABSOLUTE)
                        qWarning() << "Can't handle MOUSE_MOVE_ABSOLUTE";
                    if (raw->data.mouse.usFlags & MOUSE_VIRTUAL_DESKTOP)
                        qWarning() << "Can't handle MOUSE_VIRTUAL_DESKTOP";

                    qWarning() << "Ignored mouse input.";
                    return true;
                }
                uint distance = std::abs(raw->data.mouse.lLastX) +
                                std::abs(raw->data.mouse.lLastY);
                _distanceSinceLastTimer += distance;
                if (_leftButtonDown)
                    _distanceSinceButtonDown += distance;

                if (_distanceSinceButtonDown > 10)
                    emit userDragged();

                _timer.start(350);
            }
        }

        return true;
    }

    return false;
}
