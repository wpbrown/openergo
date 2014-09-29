#include "tracker.h"
#include "interactionsource.h"

#include <QDebug>

Tracker::Tracker(InteractionSource *source, QObject *parent) :
    QObject(parent), _source(source)
{
    connect(source, &InteractionSource::userDragged, this, &Tracker::_setUserDragged);
    connect(source, &InteractionSource::userDwelled, this, &Tracker::_autoAction);
    connect(source, &InteractionSource::userClicked, this, &Tracker::_cancelTimer);
    connect(source, &InteractionSource::dragShortcutPressed, this, &Tracker::_toggleDragDrop);

    _clickSound.setSource(QUrl("qrc:/openergo/click.wav"));
    qDebug() << _clickSound.supportedMimeTypes();
}

void
Tracker::_autoAction()
{
    if (_paused)
        return;

    qDebug() << "User dwelled.";
    if (_inDrag) {
        qDebug() << "In drag mode.";
        return;
    }

    if (_userDragged) {
        qDebug() << "Ignoring first dwell after user drag.";
        _userDragged = false;
        return;
    }

    if (_source->isLeftButtonDown()) {
        qDebug() << "User is dragging.";
        return;
    }

    if (_source->isBlockClicksDown()) {
        qDebug() << "User blocked click.";
        return;
    }

    if (_source->isDoubleClickDown()) {
        qDebug() << "Requesting double click.";
        _source->simulateClick(Qt::LeftButton, ButtonDoubleClick);
        _clickSound.play();
        return;
    }

    Qt::MouseButton button;
    if (_source->isRightClickDown()) {
        button = Qt::RightButton;
        qDebug() << "Requesting right click.";
    } else {
        button = Qt::LeftButton;
        qDebug() << "Requesting left click.";
    }
    _source->simulateClick(button, ButtonClick);
    _clickSound.play();
}

void
Tracker::_toggleDragDrop()
{
    if (!_inDrag) {
        qDebug() << "Request drag start";
        _source->simulateClick(Qt::LeftButton, ButtonDown);
        _inDrag = true;
    } else {
        qDebug() << "Request end drag";
        _source->simulateClick(Qt::LeftButton, ButtonUp);
        _inDrag = false;
    }
}

void
Tracker::_setUserDragged()
{
    qDebug() << "User initiated drag.";
    _userDragged = true;
}

void
Tracker::_cancelTimer()
{
    if (_inDrag) {
        qDebug() << "Click canceling drag.";
        _inDrag = false;
    }
}
