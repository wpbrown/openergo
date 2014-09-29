#ifndef TRACKER_H
#define TRACKER_H

#include <QSoundEffect>

class InteractionSource;

class Tracker : public QObject
{
    Q_OBJECT
public:
    explicit Tracker(InteractionSource *source, QObject *parent = 0);

private slots:
    void _cancelTimer();
    void _autoAction();
    void _toggleDragDrop();
    void _setUserDragged();

private:
    InteractionSource *_source;
    QSoundEffect _clickSound;

    bool _inDrag = false;
    bool _userDragged = false;
    bool _paused = false;
};

#endif // TRACKER_H
