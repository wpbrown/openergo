#ifndef INTERACTIONSOURCE_H
#define INTERACTIONSOURCE_H

#include <QObject>
#include <QTimer>
#include <QAbstractNativeEventFilter>

enum ButtonAction {
    ButtonDown = 0x01,
    ButtonUp = 0x02,
    ButtonClick = ButtonDown | ButtonUp,
    ButtonDoubleClick = 0x04
};

class InteractionSource : public QObject, public QAbstractNativeEventFilter
{
    Q_OBJECT

public:
    static InteractionSource &instance();

    void simulateClick(Qt::MouseButton button, ButtonAction action);
    bool isBlockClicksDown();
    bool isRightClickDown();
    bool isDoubleClickDown();
    bool isLeftButtonDown();
    bool nativeEventFilter(const QByteArray &eventType, void *message, long *result) override;

signals:
    void userDragged();
    void userDwelled();
    void userClicked();
    void dragShortcutPressed();

private slots:
    void _timerExpired();

private:
    explicit InteractionSource();
    ~InteractionSource();

    QTimer _timer;
    uint _distanceSinceLastTimer = 0;
    uint _distanceSinceButtonDown = 0;
    bool _metaDown = false;
    bool _zDown = false;
    bool _xDown = false;
    bool _leftButtonDown = false;
};


#endif // INTERACTIONSOURCE_H
