#include "interactionsource.h"

#include <QCoreApplication>
#include <QDebug>

InteractionSource &
InteractionSource::instance()
{
    static InteractionSource source;
    QCoreApplication::instance()->installNativeEventFilter(&source);
    return source;
}

bool
InteractionSource::isBlockClicksDown()
{
    return _metaDown && !(_zDown || _xDown);
}

bool
InteractionSource::isRightClickDown()
{
    return _metaDown && _zDown;
}

bool
InteractionSource::isDoubleClickDown()
{
    return _metaDown && _xDown;
}

bool
InteractionSource::isLeftButtonDown()
{
    return _leftButtonDown;
}

void
InteractionSource::_timerExpired()
{
    if (_distanceSinceLastTimer > 5)
        emit userDwelled();
    else
        qDebug() << "Ignored small move:" << _distanceSinceLastTimer << "pixels";
    _distanceSinceLastTimer = 0;
}
